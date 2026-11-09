//! Integration pins for the F06 ODF patch substrate (`patch.rs`, spec v0.2
//! §V2-F06 / D30).
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors before
//! `oxide_fw_odf::patch` existed (the `live_tdd.rs` pattern — the contract is
//! pinned from outside the crate). The committed golden fixtures under
//! `tests/fixtures/patch/` are consumed by BOTH this suite and the TS suite
//! (`packages/core/test/odf-patch.test.ts`); regenerate with
//! `cargo run -p oxide-fw-odf --example odf_fixture`.

use oxide_fw_odf::Value;
use oxide_fw_odf::fixtures;
use oxide_fw_odf::patch::{
    MAGIC_PATCH, Op, PATCH_MAX_OPS, Seg, apply, base_id, decode_patch, diff, encode_patch,
    is_patch, ops_to_value, value_to_ops,
};
use proptest::prelude::*;
use std::collections::BTreeMap;

// ---------- helpers ----------

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    )
}

fn obj_owned(pairs: Vec<(String, Value)>) -> Value {
    Value::Object(pairs.into_iter().collect())
}

fn arr(items: &[Value]) -> Value {
    Value::Array(items.to_vec())
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn n(v: f64) -> Value {
    Value::Number(v)
}

fn key(k: &str) -> Seg {
    Seg::Key(k.to_string())
}

fn idx(i: usize) -> Seg {
    Seg::Idx(i)
}

/// Error code of a failed operation (the integration.rs helper shape).
fn code_of<T>(r: oxide_fw_odf::Result<T>) -> String {
    match r {
        Err(e) => e.0.code.clone(),
        Ok(_) => panic!("expected an error"),
    }
}

/// 2000 rows of `{"i": k, "name": "row-k"}` — the spec's acceptance shape.
fn row_list(rows: usize) -> Value {
    Value::Array(
        (0..rows)
            .map(|k| obj(&[("i", n(k as f64)), ("name", s(&format!("row-{k}")))]))
            .collect(),
    )
}

// ---------- diff: the spec's structural cases ----------

#[test]
fn diff_set_at_the_root_and_nested_objects() {
    // Scalar change → one Set at the key path.
    let ops = diff(&obj(&[("a", n(1.0))]), &obj(&[("a", n(2.0))]));
    assert_eq!(
        ops,
        vec![Op::Set {
            path: vec![key("a")],
            value: n(2.0),
        }],
        "a scalar change is exactly one Set"
    );
    // Nested change → recursion by key; untouched siblings emit nothing.
    let old = obj(&[
        ("a", obj(&[("b", obj(&[("c", n(1.0))]))])),
        ("keep", s("x")),
    ]);
    let new = obj(&[
        ("a", obj(&[("b", obj(&[("c", n(2.0))]))])),
        ("keep", s("x")),
    ]);
    let ops = diff(&old, &new);
    assert_eq!(
        ops,
        vec![Op::Set {
            path: vec![key("a"), key("b"), key("c")],
            value: n(2.0),
        }],
        "a nested change is one Set at the deep path"
    );
}

#[test]
fn diff_removed_key_is_a_del() {
    let old = obj(&[("a", n(1.0)), ("gone", s("bye"))]);
    let new = obj(&[("a", n(1.0))]);
    assert_eq!(
        diff(&old, &new),
        vec![Op::Del {
            path: vec![key("gone")]
        }]
    );
}

#[test]
fn diff_append_prepend_middle_and_truncate_are_one_splice_each() {
    let base = arr(&[n(1.0), n(2.0), n(3.0)]);
    // Append.
    let ops = diff(&base, &arr(&[n(1.0), n(2.0), n(3.0), n(4.0)]));
    assert_eq!(
        ops,
        vec![Op::Splice {
            path: vec![],
            at: 3,
            remove: 0,
            insert: vec![n(4.0)],
        }]
    );
    // Prepend.
    let ops = diff(&base, &arr(&[n(0.0), n(1.0), n(2.0), n(3.0)]));
    assert_eq!(
        ops,
        vec![Op::Splice {
            path: vec![],
            at: 0,
            remove: 0,
            insert: vec![n(0.0)],
        }]
    );
    // Middle replace (one element swapped).
    let ops = diff(&base, &arr(&[n(1.0), n(9.0), n(3.0)]));
    assert_eq!(
        ops,
        vec![Op::Splice {
            path: vec![],
            at: 1,
            remove: 1,
            insert: vec![n(9.0)],
        }]
    );
    // Truncate.
    let ops = diff(&base, &arr(&[n(1.0)]));
    assert_eq!(
        ops,
        vec![Op::Splice {
            path: vec![],
            at: 1,
            remove: 2,
            insert: vec![],
        }]
    );
}

#[test]
fn diff_type_change_is_a_whole_set() {
    let old = obj(&[("v", obj(&[("k", n(1.0))]))]);
    let new = obj(&[("v", arr(&[n(1.0)]))]);
    assert_eq!(
        diff(&old, &new),
        vec![Op::Set {
            path: vec![key("v")],
            value: arr(&[n(1.0)]),
        }],
        "an object→array flip cannot recurse; the whole value is Set"
    );
}

#[test]
fn diff_map_untouched_when_equal_and_set_when_not() {
    let map = Value::Map(vec![(s("k"), n(1.0))]);
    let same = Value::Map(vec![(s("k"), n(1.0))]);
    let changed = Value::Map(vec![(s("k"), n(2.0))]);
    // Equal Map next to a changed scalar: the Map emits nothing.
    let old = obj(&[("m", map.clone()), ("x", n(1.0))]);
    let new = obj(&[("m", same), ("x", n(2.0))]);
    let ops = diff(&old, &new);
    assert_eq!(
        ops,
        vec![Op::Set {
            path: vec![key("x")],
            value: n(2.0),
        }],
        "an equal Map is untouched by the diff"
    );
    // A changed Map is one whole-value Set (no recursion into Map pairs).
    let old = obj(&[("m", map)]);
    let new = obj(&[("m", changed)]);
    assert_eq!(
        diff(&old, &new),
        vec![Op::Set {
            path: vec![key("m")],
            value: Value::Map(vec![(s("k"), n(2.0))]),
        }]
    );
}

#[test]
fn diff_no_op_is_empty() {
    let a = obj(&[
        ("rows", arr(&[obj(&[("i", n(0.0))])])),
        ("m", Value::Map(vec![(n(1.0), s("one"))])),
    ]);
    assert!(diff(&a, &a).is_empty(), "identical values diff to no ops");
}

#[test]
fn diff_deep_array_of_objects_splices_whole_changed_rows() {
    // Rows 0..4; row 2's inner key flips. The common prefix/suffix trim
    // collapses the edit into ONE Splice carrying the whole new row (the
    // spec's walk recurses through objects only, never into arrays).
    let row = |name: &str, flag: f64| obj(&[("name", s(name)), ("flag", n(flag))]);
    let old = obj(&[(
        "rows",
        arr(&[row("a", 0.0), row("b", 0.0), row("c", 0.0), row("d", 0.0)]),
    )]);
    let new = obj(&[(
        "rows",
        arr(&[row("a", 0.0), row("b", 0.0), row("c", 1.0), row("d", 0.0)]),
    )]);
    let ops = diff(&old, &new);
    assert_eq!(ops.len(), 1, "one inner-key flip is one Splice");
    assert_eq!(
        ops[0],
        Op::Splice {
            path: vec![key("rows")],
            at: 2,
            remove: 1,
            insert: vec![row("c", 1.0)],
        }
    );
}

// ---------- apply ----------

#[test]
fn apply_set_creates_a_missing_leaf_but_not_missing_intermediates() {
    let base = obj(&[("a", obj(&[("b", n(1.0))]))]);
    // Missing leaf: created.
    let out = apply(
        &base,
        &[Op::Set {
            path: vec![key("a"), key("new")],
            value: n(7.0),
        }],
    )
    .expect("leaf creation succeeds");
    assert_eq!(out, obj(&[("a", obj(&[("b", n(1.0)), ("new", n(7.0))]))]));
    // Missing intermediate: rejected.
    let e = apply(
        &base,
        &[Op::Set {
            path: vec![key("nope"), key("leaf")],
            value: n(1.0),
        }],
    )
    .expect_err("missing intermediate is an error");
    assert_eq!(e.0.code, "ODP-PATH");
}

#[test]
fn apply_del_removes_object_keys_only() {
    let base = obj(&[("a", n(1.0)), ("b", n(2.0))]);
    let out = apply(
        &base,
        &[Op::Del {
            path: vec![key("a")],
        }],
    )
    .expect("object-key del succeeds");
    assert_eq!(out, obj(&[("b", n(2.0))]));
    // Array-index del is not an object key.
    let arr_base = arr(&[n(1.0), n(2.0)]);
    let e = apply(&arr_base, &[Op::Del { path: vec![idx(0)] }]).expect_err("array del is rejected");
    assert_eq!(e.0.code, "ODP-DEL");
    // Empty path del is rejected.
    let e = apply(&base, &[Op::Del { path: vec![] }]).expect_err("empty del path is rejected");
    assert_eq!(e.0.code, "ODP-PATH");
}

#[test]
fn apply_splice_checks_the_target_and_bounds() {
    let base = obj(&[("rows", arr(&[n(1.0), n(2.0), n(3.0)]))]);
    let out = apply(
        &base,
        &[Op::Splice {
            path: vec![key("rows")],
            at: 1,
            remove: 1,
            insert: vec![n(9.0), n(8.0)],
        }],
    )
    .expect("in-bounds splice succeeds");
    assert_eq!(
        out,
        obj(&[("rows", arr(&[n(1.0), n(9.0), n(8.0), n(3.0)]))])
    );
    // Not an array.
    let e = apply(
        &obj(&[("rows", n(1.0))]),
        &[Op::Splice {
            path: vec![key("rows")],
            at: 0,
            remove: 0,
            insert: vec![],
        }],
    )
    .expect_err("splice into a scalar is rejected");
    assert_eq!(e.0.code, "ODP-SPLICE");
    // Past the end.
    let e = apply(
        &base,
        &[Op::Splice {
            path: vec![key("rows")],
            at: 2,
            remove: 5,
            insert: vec![],
        }],
    )
    .expect_err("splice past the end is rejected");
    assert_eq!(e.0.code, "ODP-SPLICE");
}

#[test]
fn apply_rejects_prototype_polluting_paths() {
    for bad in ["__proto__", "constructor"] {
        let e = apply(
            &obj(&[("a", n(1.0))]),
            &[Op::Set {
                path: vec![key("a"), key(bad)],
                value: n(1.0),
            }],
        )
        .expect_err("reserved keys are rejected in Set paths");
        assert_eq!(e.0.code, "ODP-KEY");
        let e = apply(
            &obj(&[("a", n(1.0))]),
            &[Op::Del {
                path: vec![key(bad)],
            }],
        )
        .expect_err("reserved keys are rejected in Del paths");
        assert_eq!(e.0.code, "ODP-KEY");
    }
}

#[test]
fn apply_paths_must_match_the_value_shape() {
    // Idx on an object.
    let e = apply(
        &obj(&[("a", n(1.0))]),
        &[Op::Set {
            path: vec![key("a"), idx(0)],
            value: n(1.0),
        }],
    )
    .expect_err("Idx cannot walk an object");
    assert_eq!(e.0.code, "ODP-PATH");
    // Key on an array.
    let e = apply(
        &arr(&[n(1.0)]),
        &[Op::Set {
            path: vec![key("len")],
            value: n(1.0),
        }],
    )
    .expect_err("Key cannot walk an array");
    assert_eq!(e.0.code, "ODP-PATH");
    // Path through a Map (maps are opaque to the structural diff).
    let e = apply(
        &obj(&[("m", Value::Map(vec![(s("k"), n(1.0))]))]),
        &[Op::Set {
            path: vec![key("m"), key("k")],
            value: n(2.0),
        }],
    )
    .expect_err("a Map is not walkable");
    assert_eq!(e.0.code, "ODP-PATH");
}

#[test]
fn apply_on_unchanged_ops_returns_the_base() {
    let base = obj(&[("a", n(1.0)), ("rows", arr(&[n(1.0)]))]);
    let out = apply(&base, &[]).expect("empty op list is a no-op");
    assert_eq!(out, base);
}

// ---------- property: apply(a, diff(a, b)) == b ----------

/// JSON-ish leaves (NaN excluded: `Value::Number(f64)` PartialEq makes
/// `NaN != NaN`, so a NaN leaf would pin a Set that `apply` cannot re-equal).
fn arb_leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => Just(Value::Null),
        2 => any::<bool>().prop_map(Value::Bool),
        4 => (any::<f64>()).prop_filter("no NaN", |f| !f.is_nan()).prop_map(Value::Number),
        4 => "([a-z]{0,8})".prop_map(Value::String),
    ]
}

fn arb_value() -> impl Strategy<Value = Value> {
    arb_leaf().prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            3 => prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            3 => prop::collection::btree_map("[a-z]{0,6}", inner, 0..6)
                .prop_map(Value::Object),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn apply_inverts_diff(a in arb_value(), b in arb_value()) {
        let ops = diff(&a, &b);
        let out = apply(&a, &ops).expect("diff ops always apply to their source value");
        prop_assert_eq!(out, b, "apply(a, diff(a, b)) must equal b");
    }

    #[test]
    fn diff_of_equal_values_is_empty(a in arb_value()) {
        prop_assert!(diff(&a, &a).is_empty());
    }
}

// ---------- wire shape: ops_to_value / value_to_ops ----------

#[test]
fn ops_round_trip_through_the_wire_shape() {
    let ops = vec![
        Op::Set {
            path: vec![key("a"), idx(2), key("deep")],
            value: s("v"),
        },
        Op::Del {
            path: vec![key("b")],
        },
        Op::Splice {
            path: vec![key("rows")],
            at: 1,
            remove: 2,
            insert: vec![n(1.0), n(2.0)],
        },
    ];
    let base = "ba7816bf8f01cfea";
    let wire = ops_to_value(base, &ops).expect("ops encode to the wire shape");
    // Root is `{ b: <base id>, o: [ops] }`.
    let Value::Object(m) = &wire else {
        panic!("patch root is an object");
    };
    assert_eq!(m.get("b"), Some(&s(base)));
    let Value::Array(o) = m.get("o").expect("o key") else {
        panic!("o is an array");
    };
    assert_eq!(o.len(), 3, "one wire array per op");
    // The wire root itself must be an ordinary ODF payload (encode/decode).
    let bytes = oxide_fw_odf::encode(&wire).expect("ops value encodes");
    assert_eq!(
        value_to_ops(&oxide_fw_odf::decode(&bytes).expect("decodes")).expect("parses"),
        (base.to_string(), ops)
    );
}

#[test]
fn value_to_ops_rejects_malformed_payloads() {
    // Unknown op code.
    let bad = obj_owned(vec![
        ("b".into(), s("ba7816bf8f01cfea")),
        ("o".into(), arr(&[arr(&[n(7.0), arr(&[])])])),
    ]);
    assert_eq!(code_of(value_to_ops(&bad)), "ODP-OP");
    // Missing b.
    let bad = obj_owned(vec![("o".into(), arr(&[]))]);
    assert_eq!(code_of(value_to_ops(&bad)), "ODP-SHAPE");
    // Non-array o.
    let bad = obj_owned(vec![("b".into(), s("x")), ("o".into(), n(1.0))]);
    assert_eq!(code_of(value_to_ops(&bad)), "ODP-SHAPE");
    // Fractional path index.
    let bad = obj_owned(vec![
        ("b".into(), s("x")),
        ("o".into(), arr(&[arr(&[n(0.0), arr(&[n(1.5)])])])),
    ]);
    assert_eq!(code_of(value_to_ops(&bad)), "ODP-SHAPE");
    // b is not a string.
    let bad = obj_owned(vec![("b".into(), n(1.0)), ("o".into(), arr(&[]))]);
    assert_eq!(code_of(value_to_ops(&bad)), "ODP-SHAPE");
}

// ---------- the ODP1 envelope ----------

#[test]
fn encode_patch_prefixes_odp1_and_round_trips() {
    let ops = vec![Op::Set {
        path: vec![key("a")],
        value: n(2.0),
    }];
    let bytes = encode_patch("ba7816bf8f01cfea", &ops).expect("patch encodes");
    assert_eq!(&bytes[..4], &MAGIC_PATCH, "outer magic is ODP1");
    assert!(is_patch(&bytes));
    // The inner payload is an ordinary ODF1 payload (the existing decoder
    // consumes it unchanged once the 4-byte prefix is stripped).
    assert_eq!(&bytes[4..8], b"ODF1");
    let (b, got) = decode_patch(&bytes).expect("patch decodes");
    assert_eq!(b, "ba7816bf8f01cfea");
    assert_eq!(got, ops);
    assert!(!is_patch(b"ODF1"), "a plain ODF payload is not a patch");
    assert!(!is_patch(b"OD"));
}

#[test]
fn decode_patch_rejects_bad_magic_and_truncation() {
    let ops = vec![Op::Del {
        path: vec![key("a")],
    }];
    let mut bytes = encode_patch("ba7816bf8f01cfea", &ops).expect("encodes");
    bytes[0] = b'X';
    assert_eq!(code_of(decode_patch(&bytes)), "ODP-MAGIC");
    assert_eq!(code_of(decode_patch(b"OD")), "ODP-MAGIC");
    // A bare 4-byte prefix has no ODF1 body: is_patch passes the sniff, the
    // inner decoder surfaces its own magic error.
    assert_eq!(
        code_of(decode_patch(b"ODP1")),
        "ODF-MAGIC",
        "ODF1 body missing"
    );
    // A truncated ODF1 body surfaces the decoder's own code.
    let bytes = encode_patch("ba7816bf8f01cfea", &ops).expect("encodes");
    assert_eq!(
        code_of(decode_patch(&bytes[..bytes.len() - 1])),
        "ODF-TRUNC"
    );
}

#[test]
fn base_id_is_the_sha256_digest_truncated_to_16_hex() {
    // Standard SHA-256 vector: sha256("abc") =
    // ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.
    assert_eq!(base_id(b"abc"), "ba7816bf8f01cfea");
    assert_eq!(
        base_id(b""),
        "e3b0c44298fc1c14",
        "sha256 of the empty input"
    );
    assert_eq!(base_id(b"abc").len(), 16);
    // Stable across the same bytes; distinct for different bytes.
    let full = oxide_fw_odf::encode(&obj(&[("a", n(1.0))])).expect("encodes");
    assert_eq!(base_id(&full), base_id(&full));
    assert_ne!(base_id(&full), base_id(&full[..full.len() - 1]));
}

// ---------- the committed golden fixtures (both implementations) ----------

#[test]
fn golden_patch_fixtures_apply_and_match_expected() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/patch");
    let mut seen = BTreeMap::new();
    for case in fixtures::patch_golden() {
        let base_bytes = std::fs::read(format!("{dir}/patch_{}.base.odf.bin", case.name))
            .unwrap_or_else(|e| {
                panic!(
                    "missing patch fixture for {} ({e}); run `cargo run -p oxide-fw-odf --example odf_fixture`",
                    case.name
                )
            });
        let patch_bytes = std::fs::read(format!("{dir}/patch_{}.odp.bin", case.name))
            .unwrap_or_else(|e| panic!("missing patch odp for {} ({e})", case.name));
        let expected_raw =
            std::fs::read_to_string(format!("{dir}/patch_{}.expected.json", case.name))
                .unwrap_or_else(|e| panic!("missing patch expectation for {} ({e})", case.name));
        let expected: serde_json::Value =
            serde_json::from_str(&expected_raw).expect("expected.json parses");

        // The committed base bytes encode the declared base value.
        assert_eq!(
            oxide_fw_odf::encode(&case.base).expect("base encodes"),
            base_bytes,
            "{}: base bytes drifted",
            case.name
        );
        // The committed patch decodes to (base id, ops)…
        let (b, ops) = decode_patch(&patch_bytes)
            .unwrap_or_else(|e| panic!("{}: committed patch does not decode: {e}", case.name));
        seen.insert(case.name, ops.len());
        // …whose base id matches the committed base bytes — unless the case
        // pins the mismatch contract (the client's reconnect trigger).
        let id = base_id(&base_bytes);
        if case.expect_mismatch {
            assert_ne!(
                b, id,
                "{}: the mismatch case must carry a wrong base id",
                case.name
            );
            assert_eq!(
                expected,
                serde_json::json!({ "error": "ODP-BASE" }),
                "{}: the mismatch expectation documents the refusal",
                case.name
            );
        } else {
            assert_eq!(
                b, id,
                "{}: patch base id must hash the base bytes",
                case.name
            );
            // Applying the ops to the decoded base yields the expected value.
            let base_value = oxide_fw_odf::decode(&base_bytes).expect("base decodes");
            let applied = apply(&base_value, &ops)
                .unwrap_or_else(|e| panic!("{}: apply failed: {e}", case.name));
            assert_eq!(
                expected,
                applied.to_json(),
                "{}: expected.json drifted from the applied value",
                case.name
            );
        }
    }
    // The 12 spec-named cases are all present (D30 golden list).
    let names: Vec<_> = fixtures::patch_golden().iter().map(|c| c.name).collect();
    for want in [
        "set",
        "nested-set",
        "delete",
        "append",
        "prepend",
        "middle-splice",
        "truncate",
        "type-change",
        "map-untouched",
        "deep-array-of-objects",
        "no-op",
        "base-mismatch",
    ] {
        assert!(names.contains(&want), "missing golden case {want}");
    }
    assert_eq!(names.len(), 12, "the D30 golden list has exactly 12 cases");
    assert_eq!(seen.len(), 12);
}

#[test]
fn golden_patch_fixtures_survive_the_wire_round_trip() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/patch");
    for case in fixtures::patch_golden() {
        let patch_bytes =
            std::fs::read(format!("{dir}/patch_{}.odp.bin", case.name)).expect("committed patch");
        let (b, ops) = decode_patch(&patch_bytes).expect("decodes");
        let re = encode_patch(&b, &ops).expect("re-encodes");
        assert_eq!(
            re, patch_bytes,
            "{}: patch bytes are not canonical",
            case.name
        );
    }
}

// ---------- the sending-policy acceptance bound ----------

#[test]
fn a_2000_row_append_sends_a_patch_under_five_percent_of_the_full_payload() {
    let full = oxide_fw_odf::encode(&row_list(2000)).expect("full encodes");
    let mut new_rows = row_list(2001);
    // One appended row (same shape the live loaders push).
    let appended = obj(&[("i", n(2000.0)), ("name", s("row-2000"))]);
    if let Value::Array(rows) = &mut new_rows {
        rows.push(appended);
    }
    let new_full = oxide_fw_odf::encode(&new_rows).expect("new full encodes");
    let ops = diff(&oxide_fw_odf::decode(&full).expect("decodes"), &new_rows);
    assert_eq!(ops.len(), 1, "one append is exactly one Splice");
    let patch = encode_patch(&base_id(&full), &ops).expect("patch encodes");
    let ratio = patch.len() as f64 / new_full.len() as f64;
    assert!(
        ratio < 0.05,
        "patch is {ratio:.4} of the full payload ({} vs {} bytes) — over the 5% bound",
        patch.len(),
        new_full.len()
    );
    // The spec's own sending predicate holds (patch.len()*2 < full.len()).
    assert!(patch.len() * 2 < new_full.len());
    assert!(ops.len() <= PATCH_MAX_OPS);
}

#[test]
fn the_op_cap_constant_matches_the_spec_bound() {
    assert_eq!(
        PATCH_MAX_OPS, 64,
        "D30: send ODP1 only when ops.len() <= 64"
    );
}

#[test]
fn an_empty_diff_still_round_trips_as_a_tiny_patch() {
    let v = obj(&[("a", n(1.0))]);
    let ops = diff(&v, &v);
    assert!(ops.is_empty());
    let full = oxide_fw_odf::encode(&v).expect("encodes");
    let patch = encode_patch(&base_id(&full), &ops).expect("encodes");
    let (b, got) = decode_patch(&patch).expect("decodes");
    assert_eq!(
        (b.as_str(), got.as_slice()),
        (base_id(&full).as_str(), ops.as_slice())
    );
    assert_eq!(apply(&v, &got).expect("applies"), v);
}
