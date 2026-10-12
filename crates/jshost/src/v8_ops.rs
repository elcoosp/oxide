//! V8 ops surface (spec §4.5). Rust exposes a small op namespace to JS that
//! runs inside the isolate:
//!
//! - `op_oxide_log`       — structured tracing into Rust
//! - `op_oxide_trace`     — deterministic short trace id (stub for OTLP spans)
//! - `op_oxide_serialize` — encode a JSON value with the ODF wire format (§9.5)
//!
//! The request-scoped ops (`stream_write`, `response_head`,
//! `request_body_read`, `cache_get/set/tag`, `fetch`, `ws_*`) belong to
//! per-request state that only exists once the JS render path produces
//! streaming output — they are not registered yet. A JS caller gets
//! `Op not found` from deno_core, which is the honest signal that the
//! streaming layer is not wired. See `docs/deviations.md` if kept.

use deno_core::op2;

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

/// The op declarations attached to the V8 extension.
pub fn op_decls() -> Vec<deno_core::OpDecl> {
    vec![
        op_oxide_log(),
        op_oxide_trace(),
        op_oxide_serialize(),
    ]
}
