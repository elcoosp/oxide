//! V2-E09 consumer example: the edge pure ops end-to-end on the host.
//!
//! The same four calls the wasm32 Worker makes through the `#[wasm_bindgen]`
//! bindings — match a route over the routes.json manifest, apply a redirect
//! rule through the native rules engine, round-trip an `ODP1` patch, and
//! frame an SSE event. Run on the host with:
//!
//! ```sh
//! cargo run -p oxide-fw-core-wasm --example edge_ops
//! ```
//!
//! The wasm32 build exposes the same four operations to the Cloudflare
//! Worker (see `crates/core-wasm/src/lib.rs` `bindings`).

use std::collections::BTreeMap;

use oxide_fw_core_wasm::{apply_rules, key_str, match_route, nonce_splice, odp1_apply, sse_frame};
use oxide_fw_odf::Value;
use oxide_fw_odf::patch::{diff, encode_patch};

fn obj(pairs: &[(&str, Value)]) -> Value {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

fn main() {
    // (1) Match a route over a router typegen `routes.json` manifest (§8.3).
    //     The edge reuses `RouteTree::match_path` — the same matcher the
    //     native dispatcher runs (no parallel matcher).
    let routes = r#"[
        {"id":"routes/page","pattern":"/","kind":"page"},
        {"id":"routes/blog/page","pattern":"/blog","kind":"page"},
        {"id":"routes/blog/[slug]/page","pattern":"/blog/:slug","kind":"page"}
    ]"#;
    let matched = match_route(routes, "/blog/hello").expect("routes manifest parses");
    println!("match_route('/blog/hello') -> {matched}");
    assert!(matched.contains("routes/blog/[slug]/page"));
    assert!(matched.contains("\"slug\":\"hello\""));

    // (2) Apply a redirect rule through the native rules engine (§12.1).
    //     `applyRules` in the Worker; `redirect` + `status` surface to JS.
    let rules = r#"[{"source":"/old","destination":"/new"}]"#;
    let outcome = apply_rules(rules, "/old", "").expect("rules table parses");
    println!("apply_rules('/old')       -> {outcome}");
    assert!(outcome.contains("\"redirect\":\"/new\""));
    assert!(outcome.contains("\"status\":308"));

    // (3) Round-trip an ODP1 patch (F06): diff(base, new) -> encode_patch ->
    //     odp1_apply(base_json, patch) == new. The edge reuses `odf::patch`.
    let base = obj(&[
        ("title", Value::String("v1".to_string())),
        ("count", Value::Number(3.0)),
    ]);
    let new = obj(&[
        ("title", Value::String("v2".to_string())),
        ("count", Value::Number(3.0)),
    ]);
    let ops = diff(&base, &new);
    let patch = encode_patch("base0", &ops).expect("patch encodes");
    let base_json = serde_json::to_string(&base.to_json()).expect("base serializes");
    let patched = odp1_apply(&base_json, &patch).expect("patch applies");
    println!("odp1_apply round-trip     -> {patched}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&patched).unwrap()["title"],
        "v2"
    );

    // (4) Frame an SSE event (§V2-F05 step 3) — the pure fn the live runner
    //     uses per data/ping frame, re-exported from `oxide-fw-edge-primitives`.
    let frame = sse_frame("data", Some(7), "line1\nline2");
    println!("sse_frame('data', 7, …)   -> {frame:?}");
    assert_eq!(frame, "event: data\nid: 7\ndata: line1\ndata: line2\n\n");

    // Bonus: the 32-hex cache key (D24) + the chunk-safe nonce splice, the
    // other two pure exports the Worker reuses.
    println!(
        "key_str('L0', …)          -> {} (32 hex, matches native L0)",
        key_str("L0", "routes/blog/[slug]/page", "{}", "", "build")
    );
    println!(
        "nonce_splice(…)           -> {}",
        nonce_splice("<script nonce=\"abc123\"></script>", "abc123", "SENTINEL")
    );

    println!("\nV2-E09 edge ops: all four end-to-end calls succeeded.");
}
