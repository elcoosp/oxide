//! The §20 trace-context extraction, V8-free (`trace`).
//!
//! The pure half of the trace-parent story lives here so the Node worker's
//! request-driven hosts (`loader_host`, `framework_host`, `endpoint_host`)
//! can stamp `CallInput.headers` in a `--no-default-features` build: the
//! helpers were born inside `v8_ops` (born with the V8 `fetch` op they
//! feed) but none of them touch a V8 API — they are serde + string logic
//! over the §20 shapes. The v8-gated half (the worker-thread slot, the
//! `TraceParentGuard`, the `fetch` op's propagation) stays in `v8_ops`
//! and re-exports these.
//!
//! Rules mirrored from the Node worker (parity, not breadth): malformed
//! values are DROPPED, never forwarded — garbage must not reach upstream
//! APM pipelines (S-14 / the worker's `traceparentFromPayload`).

use crate::CallInput;

/// The W3C Trace Context `traceparent` shape the Node worker's
/// `TRACEPARENT_RE` accepts, verbatim: `version-traceid-spanid-flags`
/// with 2/32/16/2 lowercase hex groups. Parity, not breadth — a value
/// the Node backend drops is dropped here too.
fn is_w3c_traceparent(value: &str) -> bool {
    let groups: [&str; 4] = match value.split('-').collect::<Vec<_>>()[..] {
        [a, b, c, d] => [a, b, c, d],
        _ => return false,
    };
    let lens = [2, 32, 16, 2];
    groups.iter().zip(lens).all(|(group, len)| {
        group.len() == len && group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    })
}

/// The request's W3C `traceparent` out of a `CallInput`, or `None` when
/// the call carries no (valid) trace context.
///
/// Extraction order (the map is the host-threaded channel — when it
/// speaks, it wins):
///
/// 1. the `headers` map's `traceparent` entry (case-insensitive key), or
/// 2. the payload's host-validated `trace.traceparent` (the value the CLI
///    derives per hop for the worker), or
/// 3. the payload's raw `headers` pairs (`[["traceparent", …]]`).
///
/// A malformed value is DROPPED, never forwarded — garbage must not reach
/// upstream APM pipelines (S-14 / the Node worker's `traceparentFromPayload`).
#[must_use]
pub fn trace_parent_from_call_input(input: &CallInput) -> Option<String> {
    for (k, v) in &input.headers {
        if k.eq_ignore_ascii_case("traceparent") {
            return is_w3c_traceparent(v).then(|| v.clone());
        }
    }
    trace_parent_from_payload_value(&input.payload)
}

/// The `CallInput.headers` map the request-driven hosts stamp at boot:
/// the validated `traceparent` (from the payload's `trace` value, else
/// its raw `headers` pairs) as the only entry, or an empty map for an
/// untraced request — nothing is fabricated.
#[must_use]
pub fn payload_trace_headers(
    payload: &serde_json::Value,
) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    if let Some(tp) = trace_parent_from_payload_value(payload) {
        map.insert("traceparent".to_string(), tp);
    }
    map
}

/// The extraction over a bare payload value (the hosts' envelopes carry
/// `trace` / `headers` in the same shape the CLI's payload whitelist
/// builds, whether the envelope is the payload itself or the render
/// input nested at `payload.input`).
fn trace_parent_from_payload_value(payload: &serde_json::Value) -> Option<String> {
    if let Some(tp) = payload
        .get("trace")
        .and_then(|t| t.get("traceparent"))
        .and_then(|v| v.as_str())
    {
        return is_w3c_traceparent(tp).then(|| tp.to_string());
    }
    if let Some(pairs) = payload.get("headers").and_then(|v| v.as_array()) {
        for pair in pairs {
            if let (Some(k), Some(v)) = (
                pair.get(0).and_then(|x| x.as_str()),
                pair.get(1).and_then(|x| x.as_str()),
            ) && k.eq_ignore_ascii_case("traceparent")
            {
                return is_w3c_traceparent(v).then(|| v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extraction is reachable V8-free: a payload's host-validated
    /// `trace.traceparent` (the CLI's per-hop shape) rides the map the
    /// hosts stamp, and a malformed value is dropped.
    #[test]
    fn payload_trace_headers_extracts_and_drops() {
        let good = serde_json::json!({
            "trace": { "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01" }
        });
        let map = payload_trace_headers(&good);
        assert_eq!(
            map.get("traceparent").map(String::as_str),
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
        );

        let malformed = serde_json::json!({
            "trace": { "traceparent": "not-a-traceparent" }
        });
        assert!(payload_trace_headers(&malformed).is_empty());
    }
}
