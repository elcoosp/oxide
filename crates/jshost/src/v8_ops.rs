//! V8 ops surface (spec §4.5). Rust exposes a small op namespace to JS that
//! runs inside the isolate:
//!
//! - `op_oxide_log`       — structured tracing into Rust
//! - `op_oxide_trace`     — deterministic short trace id (stub for OTLP spans)
//! - `op_oxide_serialize` — encode a JSON value with the ODF wire format (§9.5)
//! - `op_oxide_random`    — `len` bytes from the OS CSPRNG (ring)
//!
//! The request-scoped ops (`stream_write`, `response_head`,
//! `request_body_read`, `cache_get/set/tag`, `fetch`, `ws_*`) belong to
//! per-request state that only exists once the JS render path produces
//! streaming output — they are not registered yet. A JS caller gets
//! `Op not found` from deno_core, which is the honest signal that the
//! streaming layer is not wired. See `docs/deviations.md` if kept.

use deno_core::op2;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// The cache handler this worker thread's `JsRuntime` uses for
    /// `oxide.cacheGet/Set/Tag`. Set once at worker spawn (BUG-02) from the
    /// `V8Host`'s handler. A `JsRuntime` is `!Send` and pinned to one thread,
    /// so a thread-local is exactly the right scope — no `OpState`, no
    /// snapshot op-order churn.
    static CACHE_HANDLER: RefCell<Option<Arc<dyn oxide_cache::CacheHandler>>> =
        const { RefCell::new(None) };
}

/// Install the cache handler for the current V8 worker thread. Called once by
/// `spawn_v8_worker` before the runtime evaluates any user module.
pub fn set_cache_handler(handler: Option<Arc<dyn oxide_cache::CacheHandler>>) {
    CACHE_HANDLER.with(|c| *c.borrow_mut() = handler);
}

fn cache_handler() -> Option<Arc<dyn oxide_cache::CacheHandler>> {
    CACHE_HANDLER.with(|c| c.borrow().clone())
}

thread_local! {
    /// PERF-05: the fetch handler this V8 worker's `fetch` op delegates to.
    static FETCH_HANDLER: RefCell<Option<Arc<dyn crate::FetchHandler>>> =
        const { RefCell::new(None) };
}

thread_local! {
    /// §20: the W3C `traceparent` value for the request currently being
    /// served on this worker thread. Set by the op dispatcher from
    /// `CallInput.headers`; the `fetch` op appends it to every outgoing
    /// request so a downstream service joins the same trace. A `JsRuntime`
    /// is `!Send` and pinned to one thread, so a thread-local is the correct
    /// scope (same reasoning as `CACHE_HANDLER`/`FETCH_HANDLER`).
    static TRACE_PARENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Install the current request's `traceparent` for the worker thread.
///
/// **Not yet called from a live site.** The V8 op args do not carry the
/// inbound request headers (only the payload does), so the trace id would
/// have to be threaded through `CallInput` per op. The Node backend already
/// propagates correctly (the loader payload carries the request headers);
/// this is the V8 parity follow-up.
pub fn set_trace_parent(value: Option<String>) {
    TRACE_PARENT.with(|c| *c.borrow_mut() = value);
}

fn trace_parent() -> Option<String> {
    TRACE_PARENT.with(|c| c.borrow().clone())
}

/// Install the fetch handler for the current V8 worker thread.
pub fn set_fetch_handler(handler: Option<Arc<dyn crate::FetchHandler>>) {
    FETCH_HANDLER.with(|c| *c.borrow_mut() = handler);
}

fn fetch_handler() -> Option<Arc<dyn crate::FetchHandler>> {
    FETCH_HANDLER.with(|c| c.borrow().clone())
}

thread_local! {
    /// PERF-03: the in-flight streaming render's promise, stashed by
    /// `v8_dispatch` and driven by the worker loop after the `Chunked` stream
    /// has been handed to the caller. `None` when no stream render is running.
    pub(crate) static PENDING_STREAM: RefCell<
        Option<deno_core::v8::Global<deno_core::v8::Value>>,
    > = const { RefCell::new(None) };

    /// PERF-03: the sink a streaming V8 render pushes chunks into, keyed by
    /// the call id. Set by the worker thread before driving a streaming
    /// render; the `stream_write` op reads it. One thread = one V8 runtime, so
    /// a thread-local (not a map) is the right scope.
    static STREAM_SINK: RefCell<Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_diagnostics::OxideError>>>> =
        const { RefCell::new(None) };
}

/// Install the stream sink for the current V8 worker thread's in-flight
/// streaming render.
pub fn set_stream_sink(
    sink: Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_diagnostics::OxideError>>>,
) {
    STREAM_SINK.with(|s| *s.borrow_mut() = sink);
}

/// `oxide.streamWrite(id, bytes)` — push one render chunk to the host. Returns
/// `true` when accepted, `false` when the consumer is gone (the render should
/// stop). PERF-03.
/// `oxide.streamWrite(bytes)` as base64 (owned across the await; an async op
/// cannot borrow a `&[u8]` from the JS scope).
#[op2]
#[string]
pub async fn op_oxide_stream_write(#[string] data_b64: String) -> String {
    let tx = STREAM_SINK.with(|s| s.borrow().clone());
    let Some(tx) = tx else {
        return "false".to_string();
    };
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD
        .decode(&data_b64)
        .unwrap_or_default();
    // AWAIT the send: backpressure. A `try_send` would DROP the tail of a fast
    // render when the consumer is momentarily behind — silent data loss.
    if tx.send(Ok(bytes::Bytes::from(data))).await.is_ok() {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// `oxide.fetch(method, url, headersJson, bodyB64)` — the V8 `fetch` op.
/// Returns a JSON string `{status, headers, body_b64}` or throws. Backed by
/// the same SSRF-hardened handler as the Node worker's `fetch` shim.
#[op2]
#[string]
pub async fn op_oxide_fetch(
    #[string] method: String,
    #[string] url: String,
    #[string] headers_json: String,
    #[string] body_b64: String,
) -> Result<String, deno_error::JsErrorBox> {
    let Some(h) = fetch_handler() else {
        return Err(deno_error::JsErrorBox::generic(
            "fetch op unavailable: no FetchHandler wired",
        ));
    };
    let mut headers: Vec<(String, String)> =
        serde_json::from_str(&headers_json).unwrap_or_default();
    // §20: propagate the inbound trace to the outbound request. A caller that
    // already set its own `traceparent` wins (it knows the span better).
    if let Some(tp) = trace_parent() {
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("traceparent")) {
            headers.push(("traceparent".to_string(), tp));
        }
    }
    use base64::Engine as _;
    let body = base64::engine::general_purpose::STANDARD
        .decode(&body_b64)
        .unwrap_or_default();
    match h.fetch(&method, &url, &headers, &body).await {
        Ok(resp) => {
            let out = serde_json::json!({
                "status": resp.status,
                "headers": resp.headers,
                "body_b64": base64::engine::general_purpose::STANDARD.encode(&resp.body),
            });
            Ok(out.to_string())
        }
        Err(e) => Err(deno_error::JsErrorBox::generic(format!(
            "{}: {}",
            e.0.code, e.0.message
        ))),
    }
}

/// `oxide.log(msg)` — structured log line into Rust.
#[op2(fast)]
pub fn op_oxide_log(#[string] msg: String) {
    tracing::debug!(target: "oxide::js", "{}", msg);
}

/// `oxide.trace(name)` — deterministic short id.
#[op2]
#[string]
pub fn op_oxide_trace(#[string] name: String) -> String {
    let mut h = blake3::Hasher::new();
    h.update(name.as_bytes());
    let hex = h.finalize().to_hex().to_string();
    format!("trace-{}", &hex[..16])
}

/// `oxide.serialize(json)` — encode a JSON value to ODF bytes (§9.5).
/// Takes the value as a JSON string so we do not depend on `serde_v8`.
#[op2]
#[buffer]
pub fn op_oxide_serialize(#[string] json: String) -> Vec<u8> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
        return Vec::new();
    };
    let odf_value = oxide_odf::Value::from_json(&v);
    oxide_odf::encode(&odf_value).unwrap_or_default()
}

/// `oxide.sleep(ms)` — resolve after `ms` milliseconds. The backing timer for
/// the V8 runtime's `setTimeout`/`setInterval` (§4.6 tier 1): an async op the
/// deno_core event loop polls, so a framework's async SSR scheduler (Solid's
/// `renderToString`) can flush.
#[op2]
pub async fn op_oxide_sleep(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

/// `oxide.random(len)` — `len` bytes of cryptographically secure entropy from
/// the OS (ring's `SystemRandom`). Backs the V8 runtime's
/// `crypto.getRandomValues` (§4.6 tier 1).
///
/// This is the *only* acceptable way to provide `getRandomValues` in the V8
/// sandbox. A `Math.random`-based filler would be a security hole: it violates
/// the Web Crypto contract (a CSPRNG) and fails **silently**, so code minting a
/// session token, CSRF nonce or idempotency key would get predictable output
/// with no signal. Real entropy, or an explicit failure — never a weak fill.
///
/// On the (practically unreachable) failure of the OS RNG, returns an empty
/// `Vec`. The JS shim checks the length and throws, so a caller fails loudly
/// rather than proceeding with insufficient randomness.
#[op2]
#[buffer]
pub fn op_oxide_random(len: u32) -> Vec<u8> {
    use ring::rand::{SecureRandom, SystemRandom};
    // `getRandomValues` caps at 65536 bytes; anything larger is a caller bug.
    if len == 0 || len > 65_536 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    match SystemRandom::new().fill(&mut buf) {
        Ok(()) => buf,
        Err(_) => Vec::new(),
    }
}

/// `oxide.cacheGet(key)` — read a cache entry (§9.3/§11) as a JSON string
/// (`null` on a miss). Backs a `'use cache'` function in the V8 backend,
/// which otherwise had no cache tier at all (BUG-02).
#[op2]
#[string]
pub async fn op_oxide_cache_get(#[string] key: String) -> String {
    let Some(h) = cache_handler() else {
        return "null".to_string();
    };
    match h.get(&key).await {
        Ok(Some(entry)) => serde_json::json!({
            "status": entry.status,
            "body_b64": base64_encode(&entry.body),
        })
        .to_string(),
        _ => "null".to_string(),
    }
}

/// `oxide.cacheSet(key, status, bodyB64, tagsCsv)` — write a cache entry.
/// Returns the JSON string `"true"` / `"false"` (op2 string returns are the
/// portable form; a bare `bool` needs a fastcall signature async ops cannot
/// use). No handler → `false` (the honest "uncached" signal a caller can
/// branch on).
#[op2]
#[string]
pub async fn op_oxide_cache_set(
    #[string] key: String,
    status: u32,
    #[string] body_b64: String,
    #[string] tags_csv: String,
) -> String {
    let Some(h) = cache_handler() else {
        return "false".to_string();
    };
    let Some(body) = base64_decode(&body_b64) else {
        return "false".to_string();
    };
    let tags: Vec<String> = tags_csv
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let entry = oxide_cache::Entry {
        body,
        status: status as u16,
        headers: Vec::new(),
        tags,
        policy: oxide_cache::Policy::default(),
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        etag: String::new(),
    };
    if h.set(&key, entry).await.is_ok() {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// `oxide.cacheTag(tag)` — invalidate by tag (§11.3). Returns the number of
/// entries removed, as a JSON string.
#[op2]
#[string]
pub async fn op_oxide_cache_tag(#[string] tag: String) -> String {
    let Some(h) = cache_handler() else {
        return "0".to_string();
    };
    h.invalidate_tag(&tag, true).await.unwrap_or(0).to_string()
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// The op declarations attached to the V8 extension.
///
/// **Append-only.** deno_core keys a snapshot's op external-reference table by
/// this order, so a new op goes at the end (a snapshot built before the op
/// still restores; the new op simply has no baked reference until the next
/// `oxide build` regenerates the blob).
pub fn op_decls() -> Vec<deno_core::OpDecl> {
    vec![
        op_oxide_log(),
        op_oxide_trace(),
        op_oxide_serialize(),
        op_oxide_random(),
        op_oxide_sleep(),
        op_oxide_cache_get(),
        op_oxide_cache_set(),
        op_oxide_cache_tag(),
        op_oxide_fetch(),
        op_oxide_stream_write(),
    ]
}
