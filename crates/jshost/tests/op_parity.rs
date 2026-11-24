//! V2-A08: one shared test table proving the Node worker backend and the V8
//! backend agree on the op envelopes, per the spec's own sketch:
//!
//! > Add `crates/jshost/tests/op_parity.rs` (`#[ignore]`, engine dependent;
//! > run with `--ignored`): one fixture module per op; the same inputs go to
//! > both hosts and outputs are compared as JSON. `normalize` sorts object
//! > keys and drops timing fields.
//!
//! Acceptance (§V2-A08): `cargo test -p oxide-fw-jshost --features v8
//! --test op_parity -- --ignored` green. Engine dependent, so the target is
//! `required-features = ["v8"]` (the V8 host needs deno_core) and every test
//! is `#[ignore]`-gated — it rides the `jshost-e2e` CI job, like the other
//! isolate-booting pins.
//!
//! `endpoint:run` and `middleware:run` are the §V2-A08 gap this table pins:
//! before the parity arms existed, the V8 backend fell through to the generic
//! `__oxideDispatch` arm, which handed the raw payload to a handler expecting
//! `(request, ctx)` — the Node worker's envelopes (a built `Request`, the
//! §8.1 middleware directives, the §8.4 `Response` projection) had no V8
//! counterpart. `loader:run` (deferred) and `action:run` ride the same table
//! as the regression guard for the BUG-04 kinds.

use base64::Engine as _;
use oxide_fw_jshost::{
    CallInput, CallStream, EntryRef, JsHost, NodeHost, Pool, PoolConfig, V8Host,
};

/// The shared table: `(op kind, input fixture)`. The fixture's `module_path`
/// points into `tests/fixtures/ops/` (the `__FIXTURE_DIR__` placeholder is
/// resolved at load), and the SAME resolved payload goes to both hosts.
const CASES: &[(&str, &str)] = &[
    ("endpoint:run", "endpoint_get.json"),
    ("endpoint:run", "endpoint_post.json"),
    ("middleware:run", "middleware_locals.json"),
    ("middleware:run", "middleware_redirect.json"),
    ("loader:run", "loader_defer.json"),
    ("action:run", "action_basic.json"),
];

fn fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ops")
}

/// Load one case's input JSON and resolve its `__FIXTURE_DIR__` placeholder
/// to the real fixture path, so both hosts see byte-identical payloads.
fn case_payload(fixture: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(fixture_dir().join(fixture)).expect("fixture input");
    let mut v: serde_json::Value = serde_json::from_str(&raw).expect("fixture JSON");
    let dir = fixture_dir().to_string_lossy().to_string();
    let mp = v
        .get_mut("module_path")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    v["module_path"] = serde_json::Value::String(mp.replace("__FIXTURE_DIR__", &dir));
    v
}

/// The `EntryRef` each backend's loader builds for the op (the same shapes
/// `loader_host.rs` constructs on the pipeline path).
fn entry_for(kind: &str, payload: &serde_json::Value) -> EntryRef {
    let module = payload
        .get("module_path")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    let export = match kind {
        "endpoint:run" => payload
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("GET")
            .to_uppercase(),
        "middleware:run" => "middleware".to_string(),
        "action:run" => payload
            .get("action")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    };
    EntryRef {
        kind: kind.to_string(),
        module,
        export,
    }
}

fn call_input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "op-parity".into(),
        route: None,
    }
}

/// Spec-mandated `normalize`: sort object keys recursively and drop timing
/// fields, so JSON object ordering and wall-clock noise never mask a real
/// envelope divergence. The `streaming` transport flag is dropped with the
/// timing fields: the Node backend streams every `Response` body and the V8
/// backend may buffer a small one — the pipeline contract (`EndpointOutcome`)
/// is the status/headers/body triple either way.
fn normalize(v: &serde_json::Value) -> serde_json::Value {
    const DROPPED_KEYS: &[&str] = &["elapsedMs", "durationMs", "tookMs", "streaming"];
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            for (k, val) in map {
                if DROPPED_KEYS.contains(&k.as_str()) {
                    continue;
                }
                sorted.insert(k.clone(), normalize(val));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalize).collect())
        }
        other => other.clone(),
    }
}

/// Canonicalize one envelope the way the pipeline consumes it: any
/// `body_b64` becomes the decoded `body` text, so a `Complete` envelope and
/// a header-frame stream compare equal when the wire outcome is equal.
fn decode_body_b64(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                if k == "body_b64" {
                    if let Some(s) = val.as_str() {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(s)
                            .unwrap_or_default();
                        out.insert(
                            "body".into(),
                            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                        );
                        continue;
                    }
                }
                out.insert(k, decode_body_b64(val));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(decode_body_b64).collect())
        }
        other => other,
    }
}

/// The pipeline's view of one op call (`loader_host.rs`'s parse): a
/// `Complete` payload is the envelope JSON; a chunked payload is a header
/// frame followed by raw body bytes. Errors surface as `Err`.
async fn host_output(
    pool: &std::sync::Arc<dyn Pool>,
    kind: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let entry = entry_for(kind, &payload);
    let mut stream = pool
        .call(entry, call_input(payload))
        .await
        .map_err(|e| e.to_string())?;
    match &mut stream {
        CallStream::Complete(bytes) => {
            let v: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|e| format!("invalid JSON from {kind}: {e}"))?;
            Ok(decode_body_b64(v))
        }
        CallStream::Chunked(rx) => {
            let first = match rx.recv().await {
                Some(Ok(b)) => b,
                // The empty-stream arm mirrors `EndpointOutcome`'s default
                // (status 200, no headers, empty body).
                None => {
                    return Ok(serde_json::json!({"status": 200, "headers": [], "body": ""}));
                }
                Some(Err(e)) => return Err(e.to_string()),
            };
            let header: serde_json::Value = serde_json::from_slice(&first)
                .map_err(|e| format!("{kind} header frame invalid: {e}"))?;
            if let Some(msg) = header.get("__oxide_error").and_then(|x| x.as_str()) {
                return Err(msg.to_string());
            }
            // A chunked loader speaks NDJSON (the worker's protocol): the
            // first chunk is the initial payload line, each later chunk one
            // `{deferredSettle}` line the host folds into `deferred`
            // (`merge_settle_stream`). Endpoints stream a header frame then
            // raw body bytes instead — the payload shape decides.
            let is_ndjson = header.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            if is_ndjson {
                let mut merged = header;
                while let Some(chunk) = rx.recv().await {
                    let b = chunk.map_err(|e| e.to_string())?;
                    let line = String::from_utf8_lossy(&b);
                    for row in line.lines().filter(|l| !l.trim().is_empty()) {
                        let settle: serde_json::Value = match serde_json::from_str(row) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        let Some(s) = settle.get("deferredSettle") else {
                            continue;
                        };
                        let Some(id) = s.get("id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let value = s.get("value").cloned().unwrap_or_else(|| {
                            serde_json::json!({
                                "__oxide_deferred_error": s.get("error").and_then(|e| e.as_str()).unwrap_or("deferred rejected")
                            })
                        });
                        merged["deferred"][id] = value;
                    }
                }
                return Ok(decode_body_b64(merged));
            }
            let mut body = Vec::new();
            while let Some(chunk) = rx.recv().await {
                body.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
            }
            let mut v = header;
            v["body"] = serde_json::Value::String(String::from_utf8_lossy(&body).into_owned());
            Ok(decode_body_b64(v))
        }
    }
}

/// The §V2-A08 acceptance: for every case in the shared table, the Node
/// backend and the V8 backend produce the SAME normalized envelope.
#[tokio::test]
#[ignore = "boots a V8 isolate and a Node worker; run with --ignored"]
async fn node_and_v8_agree() {
    let node = NodeHost::new(NodeHost::worker_script_default())
        .render_pool(PoolConfig {
            size: 1,
            ..Default::default()
        })
        .expect("node pool");
    let v8 = V8Host::new()
        .render_pool(PoolConfig {
            size: 1,
            ..Default::default()
        })
        .expect("v8 pool");

    for (kind, fixture) in CASES {
        let payload = case_payload(fixture);
        let from_node = host_output(&node, kind, payload.clone())
            .await
            .unwrap_or_else(|e| panic!("node {kind}/{fixture} failed: {e}"));
        let from_v8 = host_output(&v8, kind, payload.clone())
            .await
            .unwrap_or_else(|e| panic!("v8 {kind}/{fixture} failed: {e}"));
        assert_eq!(
            normalize(&from_node),
            normalize(&from_v8),
            "op {kind} diverges between the Node and V8 backends (fixture {fixture}); \
             node: {from_node}; v8: {from_v8}"
        );
    }
}
