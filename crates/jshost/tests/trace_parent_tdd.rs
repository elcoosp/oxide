//! The §20 trace-parent threading pins (`trace_parent_tdd`): the V8 `fetch`
//! op's outbound propagation (`propagate_trace_parent`) was implemented but
//! DORMANT — `set_trace_parent` had no live call site, because the inbound
//! request's `traceparent` never reached the worker (`CallInput.headers` was
//! left empty everywhere; only the payload carried trace context, and only
//! for some call kinds). These pins hold the waking-up half:
//!
//! 1. [`trace_parent_from_call_input`] — the W3C `traceparent` extraction
//!    over a `CallInput`: the host-threaded `headers` map first (case-
//!    insensitive key), then the payload's host-validated `trace.traceparent`
//!    (the shape the CLI derives per hop), then the payload's raw
//!    `headers` pairs. Malformed values are DROPPED, never forwarded —
//!    garbage must not reach upstream APM pipelines (the Node worker's
//!    `traceparentFromPayload` rule, mirrored here for V8 parity).
//! 2. [`TraceParentGuard`] — the per-call RAII the worker loop installs:
//!    seed on construction, clear on drop (the worker runs one call at a
//!    time, so clearing — not restoring — matches the Node worker's
//!    "cleared after each dispatch" rule).
//! 3. [`propagate_trace_parent`] — the `fetch` op's append rule: a site
//!    that set its own `traceparent` wins (it knows the span better); an
//!    absent slot leaves the outgoing headers untouched.
//! 4. The wire-up: `LoaderHost` / `EndpointHost` / `FrameworkHost` stamp
//!    the validated value into `CallInput.headers` at the request-driven
//!    call sites, so the worker's fetch op can propagate it (captured
//!    through a fake `Pool`).
//!
//! The W3C shape check is the Node worker's `TRACEPARENT_RE` verbatim
//! (`2-32-16-2` lowercase hex groups) — parity, not breadth: a value the
//! Node backend would drop is dropped here too.
//!
//! These pins need no V8 runtime — the slot, the extraction and the hosts'
//! payload builders are plain Rust; the real end-to-end fetch propagation
//! rides the V8 host (CI's workspace job).

use oxide_fw_jshost::loader_host::LoaderRunArgs;
use oxide_fw_jshost::v8_ops::{
    TraceParentGuard, current_trace_parent, propagate_trace_parent, set_trace_parent,
    trace_parent_from_call_input,
};
use oxide_fw_jshost::{CallInput, CallStream, EndpointHost, EntryRef, LoaderHost, Pool, PoolStats};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The W3C spec's canonical example value (also used by the loader-host
/// tests): `00-` + 32 hex + `-` + 16 hex + `-01`.
const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// The malformed spellings the extraction must DROP (never forward).
const BAD: [&str; 4] = [
    "garbage",
    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7", // missing flags
    "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01", // uppercase hex
    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-011", // extra char
];

fn call_input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: BTreeMap::new(),
        has_body: false,
        request_id: "ex-20".to_string(),
        route: None,
    }
}

/// A fake pool that records every `CallInput` it is handed and answers a
/// minimal JSON body, so the host's payload builder runs for real while the
/// capture happens before any response parsing.
struct CapturePool {
    captured: Mutex<Vec<CallInput>>,
}

impl CapturePool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            captured: Mutex::new(Vec::new()),
        })
    }
    fn take(&self) -> Vec<CallInput> {
        std::mem::take(&mut self.captured.lock().unwrap())
    }
}

#[async_trait::async_trait]
impl Pool for CapturePool {
    async fn call(
        &self,
        _entry: EntryRef,
        req: CallInput,
    ) -> Result<CallStream, oxide_fw_diagnostics::OxideError> {
        self.captured.lock().unwrap().push(req);
        Ok(CallStream::Complete(bytes::Bytes::from(
            serde_json::to_vec(&json!({"ok": true})).unwrap_or_default(),
        )))
    }
    fn stats(&self) -> PoolStats {
        PoolStats::default()
    }
    fn abort_request(&self, _request_id: &str) {}
    async fn drain(&self) {}
}

// ---------------------------------------------------------------- extraction

#[test]
fn extraction_prefers_the_host_threaded_headers_map() {
    let mut input = call_input(json!({
        "trace": {"traceparent": "00-11111111111111111111111111111111-2222222222222222-01"},
    }));
    input
        .headers
        .insert("traceparent".to_string(), VALID.to_string());
    // The map is the explicitly-threaded channel — when it speaks, it wins
    // over the payload's derived value (the host stamped it on purpose).
    assert_eq!(trace_parent_from_call_input(&input).as_deref(), Some(VALID));
}

#[test]
fn extraction_falls_back_to_the_validated_payload_trace() {
    let input = call_input(json!({
        "route": "r",
        "trace": {"traceparent": VALID, "tracestate": "oxide=1"},
    }));
    assert_eq!(trace_parent_from_call_input(&input).as_deref(), Some(VALID));
}

#[test]
fn extraction_falls_back_to_a_raw_payload_headers_pair() {
    // S-14's fallback: the raw request headers the payload carries. Accepted
    // only when the value matches the W3C shape.
    let input = call_input(json!({
        "headers": [["cookie", "a=b"], ["traceparent", VALID]],
    }));
    assert_eq!(trace_parent_from_call_input(&input).as_deref(), Some(VALID));
}

#[test]
fn extraction_drops_a_malformed_payload_trace() {
    for bad in BAD {
        let input = call_input(json!({ "trace": {"traceparent": bad} }));
        assert_eq!(
            trace_parent_from_call_input(&input),
            None,
            "malformed `{bad}` must be dropped, not forwarded"
        );
    }
}

#[test]
fn extraction_drops_a_malformed_raw_header_pair() {
    let input = call_input(json!({
        "headers": [["traceparent", "not-a-traceparent"]],
    }));
    assert_eq!(trace_parent_from_call_input(&input), None);
}

#[test]
fn extraction_prefers_the_payload_trace_over_the_raw_pair() {
    let input = call_input(json!({
        "trace": {"traceparent": VALID},
        "headers": [["traceparent", "00-11111111111111111111111111111111-2222222222222222-01"]],
    }));
    // S-14: the host-validated value wins over the raw header.
    assert_eq!(trace_parent_from_call_input(&input).as_deref(), Some(VALID));
}

#[test]
fn extraction_reads_the_headers_map_case_insensitively() {
    let mut input = call_input(json!({}));
    input
        .headers
        .insert("TraceParent".to_string(), VALID.to_string());
    assert_eq!(trace_parent_from_call_input(&input).as_deref(), Some(VALID));
}

#[test]
fn extraction_of_an_untraced_call_input_is_none() {
    let input = call_input(json!({"route": "r", "headers": [["cookie", "a=b"]]}));
    assert_eq!(trace_parent_from_call_input(&input), None);
}

#[test]
fn extraction_is_none_for_a_payload_without_trace_or_headers() {
    let input = call_input(json!({ "module_path": "x" }));
    assert_eq!(trace_parent_from_call_input(&input), None);
}

// ------------------------------------------------------------------- the guard

#[test]
fn guard_seeds_the_slot_and_clears_on_drop() {
    let input = call_input(json!({ "trace": {"traceparent": VALID} }));
    {
        let _guard = TraceParentGuard::for_call(&input);
        assert_eq!(current_trace_parent().as_deref(), Some(VALID));
    }
    assert_eq!(current_trace_parent(), None, "the guard clears on drop");
}

#[test]
fn guard_over_an_untraced_call_clears_a_stale_slot() {
    // The worker thread is reused across calls: a previous traced call's
    // value must never leak into the next untraced one.
    set_trace_parent(Some(VALID.to_string()));
    let input = call_input(json!({}));
    {
        let _guard = TraceParentGuard::for_call(&input);
        assert_eq!(current_trace_parent(), None);
    }
    assert_eq!(current_trace_parent(), None);
}

#[test]
fn guard_clears_instead_of_restoring() {
    // The worker runs one call at a time, so the guard CLEARS (the Node
    // worker's "cleared after each dispatch" rule) rather than restoring a
    // previous value.
    set_trace_parent(Some(VALID.to_string()));
    let traced = call_input(json!({ "trace": {"traceparent": VALID} }));
    let outer = TraceParentGuard::for_call(&traced);
    let inner = TraceParentGuard::for_call(&traced);
    drop(inner);
    assert_eq!(current_trace_parent(), None, "clear, not restore");
    drop(outer);
}

// ---------------------------------------------------------------- propagation

#[test]
fn propagate_appends_the_slot_value_when_the_site_did_not_set_one() {
    set_trace_parent(Some(VALID.to_string()));
    let mut headers: Vec<(String, String)> =
        vec![("content-type".to_string(), "text/plain".into())];
    propagate_trace_parent(&mut headers);
    assert_eq!(headers[1], ("traceparent".to_string(), VALID.to_string()));
    set_trace_parent(None);
}

#[test]
fn propagate_leaves_a_site_set_traceparent_alone() {
    set_trace_parent(Some(VALID.to_string()));
    let site = "00-11111111111111111111111111111111-2222222222222222-01";
    let mut headers: Vec<(String, String)> = vec![("TraceParent".to_string(), site.to_string())];
    propagate_trace_parent(&mut headers);
    assert_eq!(headers.len(), 1, "the site's own header wins");
    assert_eq!(headers[0].1, site);
    set_trace_parent(None);
}

#[test]
fn propagate_without_a_slot_leaves_headers_untouched() {
    set_trace_parent(None);
    let mut headers: Vec<(String, String)> = vec![("accept".to_string(), "*/*".into())];
    propagate_trace_parent(&mut headers);
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, "accept");
}

// ------------------------------------------------------------------ the wire-up

#[tokio::test]
async fn loader_run_threads_the_validated_traceparent_into_callinput_headers() {
    let pool = CapturePool::new();
    let host = LoaderHost::new(pool.clone());
    let input = json!({
        "route": "/r",
        "url": "https://oxide.test/r",
        "method": "GET",
        "trace": {"traceparent": VALID, "tracestate": "oxide=1"},
    });
    let _ = host.run("routes/r.server.ts", input).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1, "the loader call reached the pool");
    assert_eq!(
        captured[0].headers.get("traceparent").map(String::as_str),
        Some(VALID),
        "CallInput.headers carries the validated traceparent"
    );
}

#[tokio::test]
async fn loader_run_falls_back_to_a_raw_request_header_pair() {
    let pool = CapturePool::new();
    let host = LoaderHost::new(pool.clone());
    let input = json!({
        "route": "/r",
        "headers": [["traceparent", VALID]],
    });
    let _ = host.run("routes/r.server.ts", input).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].headers.get("traceparent").map(String::as_str),
        Some(VALID)
    );
}

#[tokio::test]
async fn loader_run_of_an_untraced_request_leaves_the_headers_map_empty() {
    let pool = CapturePool::new();
    let host = LoaderHost::new(pool.clone());
    let input = json!({ "route": "/r", "headers": [["cookie", "a=b"]] });
    let _ = host.run("routes/r.server.ts", input).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1);
    assert!(
        captured[0].headers.is_empty(),
        "no fabricated traceparent for an untraced request"
    );
}

#[tokio::test]
async fn loader_run_drops_a_malformed_trace_instead_of_threading_it() {
    let pool = CapturePool::new();
    let host = LoaderHost::new(pool.clone());
    let input = json!({
        "route": "/r",
        "trace": {"traceparent": "garbage"},
    });
    let _ = host.run("routes/r.server.ts", input).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1);
    assert!(captured[0].headers.is_empty(), "garbage is dropped");
}

#[tokio::test]
async fn endpoint_run_threads_the_traceparent_into_callinput_headers() {
    let pool = CapturePool::new();
    let host = EndpointHost::new(pool.clone());
    let args = LoaderRunArgs {
        module_path: "routes/api.ts",
        method: "GET",
        url: "https://oxide.test/api",
        params: &serde_json::Map::new(),
        headers: &[("cookie".to_string(), "a=b".to_string())],
        body: &[],
        locals: &json!({}),
        trace: Some(&json!({"traceparent": VALID})),
    };
    let _ = host.run(args).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].headers.get("traceparent").map(String::as_str),
        Some(VALID)
    );
}

#[tokio::test]
async fn framework_render_threads_the_traceparent_into_callinput_headers() {
    let pool = CapturePool::new();
    let host = oxide_fw_jshost::FrameworkHost::new(pool.clone());
    let input = json!({
        "route": "/r",
        "url": "https://oxide.test/r",
        "trace": {"traceparent": VALID},
    });
    let _ = host.render("app.config.ts", input).await;
    let captured = pool.take();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].headers.get("traceparent").map(String::as_str),
        Some(VALID)
    );
}
