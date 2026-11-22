//! V8 ops surface (spec §4.5). Rust exposes a small op namespace to JS that
//! runs inside the isolate:
//!
//! - `op_oxide_log`       — structured tracing into Rust
//! - `op_oxide_trace`     — deterministic short trace id (stub for OTLP spans)
//! - `op_oxide_serialize` — encode a JSON value with the ODF wire format (§9.5)
//! - `op_oxide_random`    — `len` bytes from the OS CSPRNG (ring)
//! - `op_oxide_sleep`     — async sleep for the isolate
//! - `op_oxide_stream_write` — append a base64 chunk to the streamed response
//! - `op_oxide_fetch`     — the SSRF-guarded fetch bridge (§7.6)
//! - `op_oxide_cache_*`   — `cache_get`/`cache_set`/`cache_tag` (BUG-02)
//! - `op_oxide_compile_module` — the bundler/plugin module transform (§15)
//!
//! The cache ops are wired via the `CACHE_HANDLER` thread-local below; the
//! fetch op via `FETCH_HANDLER`; `compile_module` via `TRANSFORM_HANDLER`.
//! `response_head`/`request_body_read`/`ws_*` belong to per-request state
//! produced by the JS render path and land with the streaming render wiring;
//! a JS caller for those still gets `Op not found` from deno_core.

use crate::CallInput;
use crate::perm::PermissionSet;
use deno_core::op2;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// The cache handler this worker thread's `JsRuntime` uses for
    /// `oxide.cacheGet/Set/Tag`. Set once at worker spawn (BUG-02) from the
    /// `V8Host`'s handler. A `JsRuntime` is `!Send` and pinned to one thread,
    /// so a thread-local is exactly the right scope — no `OpState`, no
    /// snapshot op-order churn.
    static CACHE_HANDLER: RefCell<Option<Arc<dyn oxide_fw_cache::CacheHandler>>> =
        const { RefCell::new(None) };
}

/// Install the cache handler for the current V8 worker thread. Called once by
/// `spawn_v8_worker` before the runtime evaluates any user module.
pub fn set_cache_handler(handler: Option<Arc<dyn oxide_fw_cache::CacheHandler>>) {
    CACHE_HANDLER.with(|c| *c.borrow_mut() = handler);
}

fn cache_handler() -> Option<Arc<dyn oxide_fw_cache::CacheHandler>> {
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
/// §20: called by [`TraceParentGuard::for_call`] — the guard the worker
/// loop installs around every `V8Msg::Call` — so the `fetch` op can
/// propagate the request's W3C trace context to every outgoing request.
/// The value is the extraction over [`crate::CallInput`]
/// ([`trace_parent_from_call_input`]): the host-threaded `headers` map
/// first, then the payload's host-validated `trace.traceparent`, then the
/// payload's raw `headers` pairs. Malformed values are dropped, never
/// forwarded — the Node worker's `traceparentFromPayload` rule mirrored
/// for V8 parity.
pub fn set_trace_parent(value: Option<String>) {
    TRACE_PARENT.with(|c| *c.borrow_mut() = value);
}

/// The current worker thread's seeded `traceparent`, if any. The read
/// half of [`set_trace_parent`] — tests and embedder hosts use it to pin
/// the seed/clear cycle; the `fetch` op reads the private half directly.
#[must_use]
pub fn current_trace_parent() -> Option<String> {
    trace_parent()
}

// §20: the PURE extraction half moved to the V8-free `trace` module — the
// Node worker's request-driven hosts stamp `CallInput.headers` in a
// `--no-default-features` build too, and none of the helpers touch a V8
// API (serde + string logic over the §20 shapes). The v8-gated half (the
// thread-local slot, the guard, the `fetch` op's propagation) stays here.
pub use crate::trace::{payload_trace_headers, trace_parent_from_call_input};

/// The per-call RAII the worker loop installs around every `V8Msg::Call`:
/// seed the fetch op's slot from the call's trace context on construction,
/// clear it on drop. The worker runs one call at a time, so CLEARING —
/// not restoring — is the right hygiene (the Node worker stashes one
/// module-level value and clears it after each dispatch); a previous
/// traced call's value must never leak into the next untraced one on the
/// reused worker thread.
pub struct TraceParentGuard {
    _private: (),
}

impl TraceParentGuard {
    /// Seed the slot for `input`'s duration — the extracted value
    /// ([`trace_parent_from_call_input`]), or `None` to clear a stale one.
    #[must_use]
    pub fn for_call(input: &CallInput) -> Self {
        set_trace_parent(trace_parent_from_call_input(input));
        Self { _private: () }
    }
}

impl Drop for TraceParentGuard {
    fn drop(&mut self) {
        set_trace_parent(None);
    }
}

// V2-F19: the per-route capability permission slot — the grant the
// `op_oxide_fetch` / `op_oxide_env_get` / `op_oxide_fs_read` boundary
// checks consult. Seeded per-call by [`PermGuard::for_call`] from the
// route's [`PermissionSet`] (the dispatcher records `request_id` →
// `PermissionSet` before calling into the isolate; the guard seeds the
// thread-local from the route's grant and clears it on drop). A
// `JsRuntime` is `!Send` and pinned to one thread, so a thread-local is
// the correct scope — the same reasoning as `TRACE_PARENT`.
//
// Model 2 (the spike's soundness closure, see
// `docs/decisions/F19-permissions.md`): the isolate pool is keyed by the
// route's profile hash, so the grant is baked into the isolate and a
// detached timer (a `setTimeout` created outside the request context)
// fires against the SAME grant, never `None` and never a different
// route's. The thread-local is the per-call read; the profile-keyed pool
// is the per-isolate invariant.
thread_local! {
    static PERM_CTX: RefCell<Option<PermissionSet>> = const { RefCell::new(None) };
}

/// Install the current call's per-route permission grant for the worker
/// thread. Called by [`PermGuard::for_call`] — the RAII the worker loop
/// installs around every `V8Msg::Call` — so the gated ops can read the
/// grant at the op boundary. `None` clears a stale grant (CLEARING, not
/// restoring — the same hygiene as `set_trace_parent`).
pub fn set_perm_ctx(value: Option<PermissionSet>) {
    PERM_CTX.with(|c| *c.borrow_mut() = value);
}

/// The current worker thread's seeded permission grant, if any. The read
/// half of [`set_perm_ctx`] — tests and the consumer example use it to
/// pin the seed/clear cycle; the gated ops read the private half
/// directly.
#[must_use]
pub fn current_perm_ctx() -> Option<PermissionSet> {
    PERM_CTX.with(|c| c.borrow().clone())
}

/// The per-call RAII the worker loop installs around every
/// `V8Msg::Call` (V2-F19): seed the permission slot from the call's
/// route grant on construction, clear it on drop. Mirrors
/// [`TraceParentGuard`] exactly — the worker runs one call at a time, so
/// CLEARING (not restoring) is the right hygiene.
///
/// The grant is looked up from the route→permission registry the
/// dispatcher populates ([`crate::perm::set_route_permissions`]); a call
/// whose route has no declaration seeds the unrestricted default (v0.2
/// allow-all).
pub struct PermGuard {
    _private: (),
}

impl PermGuard {
    /// Seed the slot for `input`'s duration — the route's grant from the
    /// registry, or the unrestricted default when the route declares no
    /// permissions (the v0.2 allow-all posture).
    #[must_use]
    pub fn for_call(input: &CallInput) -> Self {
        let grant = input
            .route
            .as_deref()
            .and_then(crate::perm::route_permissions)
            .unwrap_or_default();
        set_perm_ctx(Some(grant));
        Self { _private: () }
    }
}

impl Drop for PermGuard {
    fn drop(&mut self) {
        set_perm_ctx(None);
    }
}

/// V2-F19: the net (fetch) boundary check. Reads the seeded
/// [`PermissionSet`] and returns `Ok(())` when the host is allowed,
/// `Err` with `PRM-001` when the grant denies it. The op boundary calls
/// this before delegating to the fetch handler.
fn check_net(host: &str) -> Result<(), deno_error::JsErrorBox> {
    let Some(grant) = current_perm_ctx() else {
        return Ok(()); // no grant seeded — unrestricted (the default)
    };
    if grant.is_net_allowed(host) {
        Ok(())
    } else {
        Err(deno_error::JsErrorBox::generic(format!(
            "PRM-001: route denied fetch to `{host}` — add it to config.permissions.net or remove the call"
        )))
    }
}

/// V2-F19: the env boundary check — the `process.env` Proxy's backing op
/// calls this before returning a value.
fn check_env(name: &str) -> Result<(), deno_error::JsErrorBox> {
    let Some(grant) = current_perm_ctx() else {
        return Ok(());
    };
    if grant.is_env_allowed(name) {
        Ok(())
    } else {
        Err(deno_error::JsErrorBox::generic(format!(
            "PRM-001: route denied env read `{name}` — add it to config.permissions.env or remove the call"
        )))
    }
}

/// V2-F19: the fs boundary check — the `node:fs.readFile` shim's backing
/// op calls this before reading the file.
fn check_fs(path: &str) -> Result<(), deno_error::JsErrorBox> {
    let Some(grant) = current_perm_ctx() else {
        return Ok(());
    };
    if grant.is_fs_allowed(path) {
        Ok(())
    } else {
        Err(deno_error::JsErrorBox::generic(format!(
            "PRM-001: route denied fs read `{path}` — add it to config.permissions.fs or remove the call"
        )))
    }
}

/// §20: propagate the seeded inbound trace to an outgoing request's
/// headers. A caller that already set its own `traceparent` wins (it
/// knows the span better — the case-insensitive check mirrors HTTP
/// header semantics). Without a seeded slot the headers are untouched.
pub fn propagate_trace_parent(headers: &mut Vec<(String, String)>) {
    if let Some(tp) = trace_parent()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("traceparent"))
    {
        headers.push(("traceparent".to_string(), tp));
    }
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
    /// §6.2 / §15: the plugin `transform` hook a worker's `compile_module`
    /// op applies before the Oxc passes. Mirrors `CACHE_HANDLER` /
    /// `FETCH_HANDLER`: a `JsRuntime` is `!Send`, so a thread-local is the
    /// right scope, and a fresh worker installs it before any user module
    /// runs (`spawn_v8_worker`).
    static TRANSFORM_HANDLER: RefCell<Option<Arc<dyn crate::TransformHandler>>> =
        const { RefCell::new(None) };
}

/// Install the transform handler for this V8 worker thread.
pub fn set_transform_handler(handler: Option<Arc<dyn crate::TransformHandler>>) {
    TRANSFORM_HANDLER.with(|c| *c.borrow_mut() = handler);
}

fn transform_handler() -> Option<Arc<dyn crate::TransformHandler>> {
    TRANSFORM_HANDLER.with(|c| c.borrow().clone())
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
    static STREAM_SINK: RefCell<Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_fw_diagnostics::OxideError>>>> =
        const { RefCell::new(None) };
}

thread_local! {
    /// B-03 / B2: sink for a streaming `loader:run`. The prelude emits the
    /// initial payload and one line per settled `defer()`; the worker drives
    /// the promise so timers/rejections fire after the caller already has the
    /// chunk stream.
    static LOADER_EMIT: RefCell<
        Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_fw_diagnostics::OxideError>>>,
    > = const { RefCell::new(None) };

    /// Whether the streaming loader emitted its initial payload. Used to fall
    /// back to the promise's return value for early error/redirect exits that
    /// skip the emit.
    static LOADER_EMITTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_loader_emit_sink(
    sink: Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_fw_diagnostics::OxideError>>>,
) {
    LOADER_EMIT.with(|s| *s.borrow_mut() = sink);
    LOADER_EMITTED.with(|e| e.set(false));
}

/// B-03: emit one NDJSON line from the streaming loader.
#[op2]
#[string]
pub fn op_oxide_loader_emit(#[string] line: String) -> String {
    let tx = LOADER_EMIT.with(|s| s.borrow().clone());
    let Some(tx) = tx else {
        return "false".to_string();
    };
    LOADER_EMITTED.with(|e| e.set(true));
    let mut bytes = line.into_bytes();
    bytes.push(b'\n');
    if tx.try_send(Ok(bytes::Bytes::from(bytes))).is_ok() {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// B-03: after the streaming loader's promise resolves, push its return value
/// when the initial payload was never emitted (early `error()`/`redirect()`).
pub(crate) fn finish_loader_stream(resolved: String) {
    let emitted = LOADER_EMITTED.with(|e| e.get());
    if emitted || resolved.is_empty() {
        return;
    }
    let tx = LOADER_EMIT.with(|s| s.borrow().clone());
    if let Some(tx) = tx {
        let _ = tx.try_send(Ok(bytes::Bytes::from(resolved)));
    }
}

/// Install the stream sink for the current V8 worker thread's in-flight
/// streaming render.
pub fn set_stream_sink(
    sink: Option<tokio::sync::mpsc::Sender<Result<bytes::Bytes, oxide_fw_diagnostics::OxideError>>>,
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
    // V2-F19: the net boundary check. The seeded `PermissionSet`'s
    // `is_net_allowed(host)` runs before the fetch handler — a route
    // whose grant does not list the host (or lists `Some([])`) is denied
    // with `PRM-001`. A relative URL (no host) passes (no network); the
    // SSRF guard in the fetch handler is the second layer.
    if let Some(host) = crate::perm::url_host(&url) {
        check_net(&host)?;
    }
    let Some(h) = fetch_handler() else {
        return Err(deno_error::JsErrorBox::generic(
            "fetch op unavailable: no FetchHandler wired",
        ));
    };
    let mut headers: Vec<(String, String)> =
        serde_json::from_str(&headers_json).unwrap_or_default();
    // §20: propagate the inbound trace to the outbound request. A caller
    // that already set its own `traceparent` wins (it knows the span
    // better) — the append rule rides [`propagate_trace_parent`] so the
    // pins and the example hold the one behavior the op ships.
    propagate_trace_parent(&mut headers);
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
    let odf_value = oxide_fw_odf::Value::from_json(&v);
    oxide_fw_odf::encode(&odf_value).unwrap_or_default()
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
    let entry = oxide_fw_cache::Entry {
        body,
        status: status as u16,
        headers: Vec::new(),
        tags,
        policy: oxide_fw_cache::Policy::default(),
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

/// §6.2 / §15: the plain (non-`#[op2]`) implementation of the V8
/// `compile_module` op. Extracted so it can be unit-tested directly —
/// `#[op2]` turns the decorated wrapper into a 0-arg `OpDecl` generator,
/// which cannot be invoked as a normal async function.
/// G1: a synchronous entry to the same transform + lower pipeline the
/// `compile_module` op runs, for the V8 module loader.
///
/// The loader's `load()` is a sync trait method on the runtime thread, so it
/// cannot `.await`. The work runs on a scoped thread with its own
/// current-thread runtime — the same bridge the CLI's plugin hooks use. The
/// tooling pool has its own runtime (fixed in bdfe82f), so blocking here does
/// not starve it.
pub fn compile_module_sync(path: String, code: String, lang: String) -> crate::Result<String> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| crate::err("JSH-V8COMPILE", format!("bridge runtime: {e}")))?;
                // `compile_module_impl` returns the op's JSON string — a
                // `{code: ...}` object on success, an error object otherwise.
                let json = rt.block_on(compile_module_impl(path, code, lang));
                match serde_json::from_str::<serde_json::Value>(&json) {
                    Ok(v) => {
                        if let Some(s) = v.get("code").and_then(|s| s.as_str()) {
                            Ok(s.to_string())
                        } else if let Some(m) = v.get("error").and_then(|m| m.as_str()) {
                            Err(crate::err("JSH-V8COMPILE", m.to_string()))
                        } else {
                            Err(crate::err(
                                "JSH-V8COMPILE",
                                format!("unexpected compile result: {json}"),
                            ))
                        }
                    }
                    Err(_) => Err(crate::err(
                        "JSH-V8COMPILE",
                        format!("compile result is not JSON: {json}"),
                    )),
                }
            })
            .join()
            .unwrap_or_else(|_| {
                Err(crate::err(
                    "JSH-V8COMPILE",
                    "compile bridge thread panicked",
                ))
            })
    })
}

pub async fn compile_module_impl(path: String, code: String, lang: String) -> String {
    // B-13: memoise by (path, lang, content hash). The Node worker gates
    // recompiles by source hash too; this keeps the Oxc pass from re-running
    // when a V8 caller asks for the same module twice. Capped so plugin
    // transforms of many distinct files cannot grow the map without bound.
    static COMPILE_MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::OnceLock::new();
    let memo = COMPILE_MEMO.get_or_init(Default::default);
    let key = format!(
        "{path}\u{0}{lang}\u{0}{}",
        blake3::hash(code.as_bytes()).to_hex()
    );
    if let Some(hit) = memo.lock().unwrap().get(&key).cloned() {
        return hit;
    }
    let source = if let Some(h) = transform_handler() {
        match h.transform(&path, &code).await {
            Ok(s) => s,
            Err(e) => {
                return serde_json::json!({
                    "code": e.0.code,
                    "error": format!("plugin transform failed for {path}: {}", e.0.message),
                })
                .to_string();
            }
        }
    } else {
        code
    };
    let lang_str = match lang.as_str() {
        "tsx" => "tsx",
        "jsx" => "jsx",
        "js" | "mjs" => "mjs",
        _ => "ts",
    };
    let input = oxide_fw_compiler::TransformInput {
        path: path.clone(),
        code: source,
        env: oxide_fw_compiler::Env::Ssr,
        lang: lang_str,
        build_secret: String::new(),
        dev: true,
        jsx_import_source: None,
    };
    let result = match oxide_fw_compiler::OxcTransformer.transform_passes_only(&input) {
        Ok(out) => serde_json::json!({ "code": out }).to_string(),
        Err(e) => serde_json::json!({
            "code": e.0.code,
            "error": format!("compile_module failed for {path}: {}", e.0.message),
        })
        .to_string(),
    };
    {
        let mut m = memo.lock().unwrap();
        if m.len() >= 1024 {
            // Simple bound: drop the memo rather than track LRU order. A
            // dropped memo only costs a recompile, never correctness.
            m.clear();
        }
        m.insert(key, result.clone());
    }
    result
}

/// §6.2 / §15: compile a user server module (TS/TSX/MJS) — the V8 parity
/// of the Node worker's `compile_server_module` path. Applies the plugin
/// `transform` hook FIRST (matching the Node `Op::CompileModule` arm), then
/// runs the Oxc passes; the emitted code stays in source form (the loader
/// transports it as `.ts`).
///
/// Returns a JSON object: `{ "code": "<compiled>" }` on success,
/// `{ "code": "<ERR-CODE>", "error": "<message>" }` on failure. The
/// prelude throws when `error` is present.
#[op2]
#[string]
pub async fn op_oxide_compile_module(
    #[string] path: String,
    #[string] code: String,
    #[string] lang: String,
) -> String {
    compile_module_impl(path, code, lang).await
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// V2-F19: `oxide.envGet(name)` — the `process.env` Proxy's backing op.
/// The V8 bootstrap wraps `globalThis.process.env` in a `Proxy` whose
/// `get` trap calls this op, so every env-var read flows through the
/// permission boundary. Returns the value as a JSON string (`null` when
/// the var is unset, `"value"` when set). A denied read throws
/// `PRM-001`.
///
/// The V8 runtime's `process.env` is a minimal shim (`{ NODE_ENV:
/// 'production' }`); this op is the gate the per-route grant checks
/// against. On a deny the op throws — the Proxy's `get` trap surfaces the
/// throw as a property-read failure, the honest signal.
#[op2]
#[string]
pub fn op_oxide_env_get(#[string] name: String) -> String {
    // V2-F19: the env boundary check. Throws (via the op's error path)
    // when the grant denies the name.
    if let Err(e) = check_env(&name) {
        // op2 sync ops cannot return `Result`; surface the deny as a
        // JSON error object the Proxy's get-trap throws. The shape
        // mirrors the compile_module op's error arm.
        return serde_json::json!({ "error": e.to_string() }).to_string();
    }
    // The V8 shim's env is `{ NODE_ENV: 'production' }` plus any vars
    // the host threaded via `CallInput`. For v0.2 the env surface is
    // minimal; the permission gate is the substrate that ships — the
    // full env read-through rides a later tier (the spec's §4.6 tier 2).
    match name.as_str() {
        "NODE_ENV" => serde_json::to_string("production").unwrap(),
        _ => serde_json::Value::Null.to_string(),
    }
}

/// V2-F19: `oxide.fsRead(path)` — the `node:fs.readFile` shim's backing
/// op. The V8 `node:fs` module's `readFile`/`readFileSync` route through
/// this op so every file read flows through the permission boundary.
/// Returns the file bytes as base64 (or a JSON error object on a deny).
#[op2]
#[string]
pub fn op_oxide_fs_read(#[string] path: String) -> String {
    // V2-F19: the fs boundary check.
    if let Err(e) = check_fs(&path) {
        return serde_json::json!({ "error": e.to_string() }).to_string();
    }
    // The V8 runtime does not expose a real fs (§4.6: a `node:fs` call
    // is loud — "set server.backend to node"). The permission gate is
    // the substrate; the real read-through rides the Node backend. A
    // granted read on the V8 backend returns the honest "not available"
    // signal rather than fabricating bytes.
    serde_json::json!({
        "error": "JSH-NODESHIM: node:fs.readFile is not available in the V8 runtime; set server.backend to node (§4.6)"
    })
    .to_string()
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
        // Appended in §6.2 / §15 — a snapshot built before this op still
        // restores (deno_core only compares against the baked table; the
        // new op simply has no external reference until the next build).
        op_oxide_compile_module(),
        // B-03: streaming loader settle lines (append-only, same snapshot
        // compatibility rule as above).
        op_oxide_loader_emit(),
        // V2-F19: the per-route capability permission ops (append-only,
        // same snapshot compatibility rule). The `process.env` Proxy and
        // the `node:fs.readFile` shim route through these so every
        // env/fs read flows through the permission boundary.
        op_oxide_env_get(),
        op_oxide_fs_read(),
    ]
}

#[cfg(test)]
mod compile_module_tests {
    use super::*;

    /// §6.2 / §15: the V8 `compile_module` op runs the Oxc passes over a
    /// raw TS module. Load-bearing for the prelude bridge — if the op were
    /// missing or its ABI wrong, this test fails.
    #[tokio::test]
    async fn op_oxide_compile_module_roundtrips() {
        set_transform_handler(None); // explicit: no plugin transform
        let out = compile_module_impl(
            "src/routes/index/page.server.ts".into(),
            "export const n: number = 1;\nexport async function load() { return { n }; }\n".into(),
            "ts".into(),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&out).expect("JSON response");
        assert!(
            v.get("error").is_none(),
            "no error on a clean TS module: {v}"
        );
        let code = v.get("code").and_then(|c| c.as_str()).expect("code field");
        assert!(
            code.contains("load"),
            "load fn survives the Oxc passes: {code}"
        );
    }
}
