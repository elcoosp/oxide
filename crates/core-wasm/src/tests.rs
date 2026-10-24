//! TDD suite for oxide-core-wasm (spec §2 D18, §18 cloudflare target).

use super::*;
use oxide_fw_odf::Value;
use std::collections::BTreeMap;

// ------------------------------------------------------------------ merge_head

#[test]
fn merge_head_inner_title_wins_and_keeps_outer_position() {
    let outer = vec![
        ("title".to_string(), "Outer".to_string()),
        ("description".to_string(), "d".to_string()),
    ];
    let inner = vec![("title".to_string(), "Inner".to_string())];
    assert_eq!(
        merge_head(outer, inner),
        vec![
            ("title".to_string(), "Inner".to_string()),
            ("description".to_string(), "d".to_string()),
        ]
    );
}

#[test]
fn merge_head_title_only_in_outer_is_kept() {
    let outer = vec![("title".to_string(), "Only".to_string())];
    assert_eq!(
        merge_head(outer, vec![]),
        vec![("title".to_string(), "Only".to_string())]
    );
}

#[test]
fn merge_head_title_only_in_inner_is_appended() {
    let outer = vec![("description".to_string(), "d".to_string())];
    let inner = vec![("title".to_string(), "Inner".to_string())];
    assert_eq!(
        merge_head(outer, inner),
        vec![
            ("description".to_string(), "d".to_string()),
            ("title".to_string(), "Inner".to_string()),
        ]
    );
}

#[test]
fn merge_head_meta_override_moves_to_inner_order() {
    // Mirrors render::HeadTags::merge: the overridden outer entry is dropped and the
    // inner entry is appended in inner order after the surviving outer entries.
    let outer = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
    ];
    let inner = vec![("a".to_string(), "9".to_string())];
    assert_eq!(
        merge_head(outer, inner),
        vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "9".to_string()),
        ]
    );
}

#[test]
fn merge_head_preserves_outer_order_and_appends_inner() {
    let outer = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
    ];
    let inner = vec![
        ("c".to_string(), "3".to_string()),
        ("b".to_string(), "8".to_string()),
    ];
    assert_eq!(
        merge_head(outer, inner),
        vec![
            ("a".to_string(), "1".to_string()),
            ("c".to_string(), "3".to_string()),
            ("b".to_string(), "8".to_string()),
        ]
    );
}

#[test]
fn merge_head_empty_inner_is_identity() {
    let outer = vec![
        ("title".to_string(), "T".to_string()),
        ("og:title".to_string(), "v".to_string()),
    ];
    assert_eq!(merge_head(outer.clone(), vec![]), outer);
}

// ------------------------------------------------------------------ encode_odf

#[test]
fn encode_odf_roundtrips_an_object() {
    let mut obj = BTreeMap::new();
    obj.insert("n".to_string(), Value::Number(1.5));
    obj.insert("s".to_string(), Value::String("oxide".to_string()));
    obj.insert(
        "arr".to_string(),
        Value::Array(vec![Value::Bool(true), Value::Null]),
    );
    let bytes = encode_odf(&Value::Object(obj.clone())).unwrap();
    assert_eq!(&bytes[..4], b"ODF1", "wire magic (§9.5)");
    assert_eq!(oxide_fw_odf::decode(&bytes).unwrap(), Value::Object(obj));
}

#[test]
fn encode_odf_is_deterministic_and_error_carrying() {
    let mut obj = BTreeMap::new();
    obj.insert("k".to_string(), Value::String("v".to_string()));
    assert_eq!(
        encode_odf(&Value::Object(obj.clone())).unwrap(),
        encode_odf(&Value::Object(obj.clone())).unwrap()
    );
    // Prototype-polluting object keys are refused with a stable code (§17.1 via ODF).
    let mut evil = BTreeMap::new();
    evil.insert("__proto__".to_string(), Value::String("x".to_string()));
    let e = encode_odf(&Value::Object(evil)).unwrap_err();
    assert_eq!(e.0.code, "ODF-KEY", "stable error code present");
}

// -------------------------------------------------------------- edge_cache_key

#[test]
fn edge_cache_key_is_deterministic_and_differs_by_inputs() {
    let a = edge_cache_key("/blog", ":slug", "", "b1");
    assert_eq!(
        a,
        edge_cache_key("/blog", ":slug", "", "b1"),
        "deterministic"
    );
    assert_ne!(
        a,
        edge_cache_key("/blog", ":slug", "", "b2"),
        "build id matters"
    );
    assert_ne!(
        a,
        edge_cache_key("/blog", ":slug", "v=2", "b1"),
        "vary matters"
    );
    assert_ne!(
        a,
        edge_cache_key("/other", ":slug", "", "b1"),
        "route matters"
    );
    assert_ne!(a, edge_cache_key("/blog", ":id", "", "b1"), "params matter");
}

#[test]
fn edge_cache_key_matches_blake3_hex_layout() {
    // 64 lowercase hex chars — the same layout blake3 produces (see the
    // blake3-equivalence note on `blake3_stub`).
    let key = edge_cache_key("/x", "", "", "");
    assert_eq!(key.len(), 64, "64-char hex layout");
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        key.chars().all(|c| !c.is_ascii_uppercase()),
        "lowercase hex"
    );
}

// -------------------------------------------------------- edge_manifest_routes

#[test]
fn edge_manifest_routes_reads_router_typegen_output() {
    // Shape produced by oxide-router typegen (.oxide/types/routes.json, §8.3).
    let routes_json = serde_json::json!([
        { "id": "routes/layout", "pattern": "/", "kind": "layout" },
        { "id": "routes/index/page", "pattern": "/", "kind": "page", "renderMode": "static" },
        { "id": "routes/blog/[slug]/page", "pattern": "/blog/:slug", "kind": "page" }
    ]);
    let routes = edge_manifest_routes(&routes_json).unwrap();
    assert_eq!(
        routes,
        vec!["/", "/blog/:slug"],
        "patterns in document order, deduped"
    );
}

#[test]
fn edge_manifest_routes_wrapped_object_and_errors() {
    let wrapped = serde_json::json!({ "routes": [ { "pattern": "/a" }, { "pattern": "/b" } ] });
    assert_eq!(
        edge_manifest_routes(&wrapped).unwrap(),
        vec!["/a".to_string(), "/b".to_string()]
    );
    let bad = serde_json::json!({ "something": "else" });
    let e = edge_manifest_routes(&bad).unwrap_err();
    assert_eq!(e.0.code, "EDGE-101", "stable code for a malformed manifest");
    assert!(
        edge_manifest_routes(&serde_json::json!([]))
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------- decode_odf

#[test]
fn decode_odf_round_trips_encode_odf() {
    let v = Value::Object(BTreeMap::from([
        ("title".to_string(), Value::String("Hi".into())),
        ("n".to_string(), Value::Number(42.0)),
        ("ok".to_string(), Value::Bool(true)),
    ]));
    let bytes = encode_odf(&v).expect("encode");
    let back = decode_odf(&bytes).expect("decode");
    assert_eq!(back, v, "encode -> decode is lossless");
}

#[test]
fn decode_odf_rejects_non_odf_bytes() {
    let err = decode_odf(b"not odf").expect_err("must reject");
    assert!(
        err.0.code.starts_with("ODF-"),
        "error carries an ODF code: {}",
        err.0.code
    );
}
