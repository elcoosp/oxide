//! The §20 trace-parent surface, walked (`cargo run -p oxide-fw-jshost --example trace_propagation`).
//!
//! The V8 `fetch` op's outbound propagation was implemented but DORMANT —
//! `set_trace_parent` had no live call site. The chain now runs end to end:
//! the request-driven hosts stamp the validated `traceparent` into
//! `CallInput.headers`, the worker loop seeds the slot through
//! [`TraceParentGuard`] around every call (clearing it on drop), and the
//! `fetch` op appends the slot's value to every outgoing request unless the
//! site set its own.
//!
//! This example walks the three seams without spawning a worker — the
//! extraction, the guard cycle and the append rule are plain Rust; the real
//! end-to-end fetch propagation rides the V8 host (CI's workspace job).

use oxide_fw_jshost::CallInput;
use oxide_fw_jshost::v8_ops::{
    TraceParentGuard, current_trace_parent, propagate_trace_parent, set_trace_parent,
    trace_parent_from_call_input,
};
use std::collections::BTreeMap;

/// The W3C spec's canonical example value — the shape the extraction
/// accepts (2-32-16-2 lowercase hex groups, the Node worker's
/// `TRACEPARENT_RE` verbatim).
const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn call_input(payload: serde_json::Value, headers: BTreeMap<String, String>) -> CallInput {
    CallInput {
        payload,
        headers,
        has_body: false,
        request_id: "ex-20".to_string(),
        route: None,
    }
}

/// The extraction: where the worker loop finds the request's trace context
/// and what it refuses to forward.
fn walk_extraction() {
    println!("== the extraction (trace_parent_from_call_input) ==");
    let mut host_threaded = call_input(
        serde_json::json!({ "trace": {"traceparent": "00-11111111111111111111111111111111-2222222222222222-01"} }),
        BTreeMap::new(),
    );
    host_threaded
        .headers
        .insert("traceparent".to_string(), VALID.to_string());
    println!(
        "  host-threaded map wins:  {:?}",
        trace_parent_from_call_input(&host_threaded)
    );

    let payload_trace = call_input(
        serde_json::json!({ "trace": {"traceparent": VALID, "tracestate": "oxide=1"} }),
        BTreeMap::new(),
    );
    println!(
        "  payload trace value:     {:?}",
        trace_parent_from_call_input(&payload_trace)
    );

    let raw_pair = call_input(
        serde_json::json!({ "headers": [["cookie", "a=b"], ["traceparent", VALID]] }),
        BTreeMap::new(),
    );
    println!(
        "  raw payload pair:        {:?}",
        trace_parent_from_call_input(&raw_pair)
    );

    let garbage = call_input(
        serde_json::json!({ "trace": {"traceparent": "not-a-traceparent"} }),
        BTreeMap::new(),
    );
    println!(
        "  malformed → dropped:     {:?} (never forwarded)",
        trace_parent_from_call_input(&garbage)
    );

    let untraced = call_input(serde_json::json!({ "route": "/r" }), BTreeMap::new());
    println!(
        "  untraced request:        {:?} (nothing fabricated)",
        trace_parent_from_call_input(&untraced)
    );
}

/// The guard cycle: the worker loop seeds the slot around every call and
/// the drop clears it, so a reused worker thread never leaks one request's
/// trace into the next.
fn walk_guard() {
    println!("== the guard (TraceParentGuard) ==");
    let traced = call_input(
        serde_json::json!({ "trace": {"traceparent": VALID} }),
        BTreeMap::new(),
    );
    {
        let _guard = TraceParentGuard::for_call(&traced);
        println!("  during the call: {:?}", current_trace_parent());
    }
    println!(
        "  after the drop:  {:?} (clear, not restore)",
        current_trace_parent()
    );

    // An untraced call over a stale slot clears it the same way.
    set_trace_parent(Some(VALID.to_string()));
    let untraced = call_input(serde_json::json!({}), BTreeMap::new());
    let _guard = TraceParentGuard::for_call(&untraced);
    println!(
        "  untraced call over a stale slot: {:?}",
        current_trace_parent()
    );
    drop(_guard);
}

/// The append rule the `fetch` op ships: the slot's value joins every
/// outgoing request unless the site set its own.
fn walk_propagate() {
    println!("== the fetch op's append (propagate_trace_parent) ==");
    set_trace_parent(Some(VALID.to_string()));
    let mut outgoing: Vec<(String, String)> =
        vec![("content-type".into(), "application/json".into())];
    propagate_trace_parent(&mut outgoing);
    println!("  slot hit:   {outgoing:?}");

    let mut site_set: Vec<(String, String)> = vec![(
        "TraceParent".into(),
        "00-11111111111111111111111111111111-2222222222222222-01".into(),
    )];
    propagate_trace_parent(&mut site_set);
    println!("  site wins:  {site_set:?} (untouched)");

    set_trace_parent(None);
    let mut untraced: Vec<(String, String)> = vec![("accept".into(), "*/*".into())];
    propagate_trace_parent(&mut untraced);
    println!("  no slot:    {untraced:?} (untouched)");
}

fn main() {
    walk_extraction();
    println!();
    walk_guard();
    println!();
    walk_propagate();
}
