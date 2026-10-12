//! Integration tests for the ODF wire format (spec §9.5, hardening §17.1).
//!
//! Covers: roundtrips (bit-exact, incl. NaN/-0.0), structural sharing, golden fixture
//! stability, decoder hardening rejection codes, `encode_with_shared`, ref resolution
//! (cycles / depth guard) and a seeded fuzz loop that must never panic.

use oxide_odf::fixtures;
use oxide_odf::{
    bitexact_eq, cyclic_fixture, decode, decode_with_shared, encode, encode_to_bytes,
    encode_with_shared, resolve_refs, resolve_refs_with, TypedArrayKind, Value, MAX_BYTES,
    MAX_DEPTH, MAX_REFS,
};
use serde_json::json;
use std::collections::BTreeMap;

// ---------- helpers ----------

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn obj_owned(pairs: Vec<(String, Value)>) -> Value {
    Value::Object(pairs.into_iter().collect())
}

fn arr(items: &[Value]) -> Value {
    Value::Array(items.to_vec())
}

fn ta(kind: TypedArrayKind, bytes: &[u8]) -> Value {
    Value::TypedArray(kind, bytes.to_vec())
}

fn le_i16(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_u32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_f64(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_i64(v: &[i64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_u64(v: &[u64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Error code of a failed operation (test helper).
fn code_of<T>(r: oxide_odf::Result<T>) -> String {
    match r {
        Err(e) => e.0.code.clone(),
        Ok(_) => panic!("expected an error"),
    }
}

fn push_varu(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn magic_buf() -> Vec<u8> {
    b"ODF1".to_vec()
}

// ---------- 1. roundtrip property (~44 representative values) ----------

fn roundtrip_suite() -> Vec<(&'static str, Value, bool)> {
    // third field: true when plain `decode` must return the value unchanged
    // (no user refs, no structural duplicates eligible for sharing).
    vec![
        ("null", Value::Null, true),
        ("undefined", Value::Undefined, true),
        ("bool-true", Value::Bool(true), true),
        ("bool-false", Value::Bool(false), true),
        ("number-zero", Value::Number(0.0), true),
        ("number-neg-zero", Value::Number(-0.0), true),
        ("number-nan", Value::Number(f64::NAN), true),
        ("number-inf", Value::Number(f64::INFINITY), true),
        ("number-neg-inf", Value::Number(f64::NEG_INFINITY), true),
        ("number-max", Value::Number(f64::MAX), true),
        ("number-min-subnormal", Value::Number(5e-324), true),
        ("number-int", Value::Number(42.0), true),
        ("number-neg", Value::Number(-273.15), true),
        ("bigint-zero", Value::BigInt("0".into()), true),
        (
            "bigint-huge",
            Value::BigInt("123456789012345678901234567890123456789012345678901234567890".into()),
            true,
        ),
        (
            "bigint-negative",
            Value::BigInt("-98765432109876543210987654321098765432109876543210".into()),
            true,
        ),
        ("string-empty", Value::String(String::new()), true),
        (
            "string-unicode",
            Value::String("héllo 🌍 日本語 ça va".into()),
            true,
        ),
        (
            "string-escapes",
            Value::String("line\nbreak \"quoted\" \\back\\\ttab".into()),
            true,
        ),
        (
            "string-controls",
            Value::String("\u{0}\u{1}\u{1f}ctrl".into()),
            true,
        ),
        ("date-zero", Value::Date(0), true),
        ("date-max", Value::Date(i64::MAX), true),
        ("date-min", Value::Date(-62135596800000), true),
        (
            "url",
            Value::Url("https://oxide.rs/docs?a=1&b=ü#frag".into()),
            true,
        ),
        (
            "regexp",
            Value::RegExp("^a.*b$\\d".into(), "gimsu".into()),
            true,
        ),
        (
            "regexp-empty",
            Value::RegExp(String::new(), String::new()),
            true,
        ),
        ("array-empty", Value::Array(vec![]), true),
        ("object-empty", Value::Object(BTreeMap::new()), true),
        ("map-empty", Value::Map(vec![]), true),
        ("set-empty", Value::Set(vec![]), true),
        (
            "array-nested",
            arr(&[
                arr(&[
                    Value::Number(1.0),
                    arr(&[Value::Number(2.0), arr(&[Value::Number(3.0)])]),
                ]),
                Value::String("x".into()),
            ]),
            true,
        ),
        (
            "object-unicode-keys",
            obj(&[
                ("z", Value::Number(1.0)),
                ("a", Value::Number(2.0)),
                ("Ä", Value::Number(3.0)),
                ("0", Value::Number(4.0)),
            ]),
            true,
        ),
        (
            "map-mixed-keys",
            Value::Map(vec![
                (Value::Null, Value::Number(1.0)),
                (Value::Number(2.0), Value::String("two".into())),
                (Value::String("s".into()), Value::Null),
                (Value::Bool(true), arr(&[Value::Number(1.0)])),
                (Value::Date(7), Value::Undefined),
            ]),
            true,
        ),
        (
            "set-mixed",
            Value::Set(vec![
                Value::Null,
                Value::Undefined,
                Value::Number(1.0),
                Value::String("1".into()),
                Value::Array(vec![]),
            ]),
            true,
        ),
        ("ta-u8", ta(TypedArrayKind::U8, &[0, 1, 254, 255]), true),
        ("ta-i8", ta(TypedArrayKind::I8, &[255, 0, 127]), true),
        (
            "ta-u8clamped",
            ta(TypedArrayKind::U8Clamped, &[0, 128, 255]),
            true,
        ),
        (
            "ta-i16",
            ta(TypedArrayKind::I16, &le_i16(&[-2, -1, 0, 1, 2])),
            true,
        ),
        (
            "ta-u32",
            ta(TypedArrayKind::U32, &le_u32(&[0xFFFF_FFFF, 1])),
            true,
        ),
        (
            "ta-f32",
            ta(TypedArrayKind::F32, &le_f32(&[1.5, -0.25, f32::INFINITY])),
            true,
        ),
        (
            "ta-f64-specials",
            ta(
                TypedArrayKind::F64,
                &le_f64(&[1.5, -2.25, f64::NAN, f64::NEG_INFINITY, -0.0]),
            ),
            true,
        ),
        (
            "ta-bigint64",
            ta(TypedArrayKind::BigInt64, &le_i64(&[i64::MIN, i64::MAX])),
            true,
        ),
        (
            "ta-biguint64",
            ta(TypedArrayKind::BigUint64, &le_u64(&[u64::MAX, 1])),
            true,
        ),
        (
            "error-plain",
            Value::Error {
                name: "TypeError".into(),
                message: "boom".into(),
                stack: None,
            },
            true,
        ),
        (
            "error-stack",
            Value::Error {
                name: "RangeError".into(),
                message: "out of range".into(),
                stack: Some("at f (x.ts:1:1)".into()),
            },
            true,
        ),
        ("deferred", Value::Deferred { id: 7 }, true),
        (
            "object-50-keys",
            obj_owned(
                (0..50)
                    .map(|i| (format!("key{i:02}"), Value::Number(i as f64)))
                    .collect(),
            ),
            true,
        ),
        (
            "array-with-nan-inside",
            arr(&[
                Value::Number(f64::NAN),
                obj(&[("x", Value::Number(-0.0))]),
                Value::Number(f64::INFINITY),
            ]),
            true,
        ),
        // needs resolve (structural duplicate → Ref on the wire)
        (
            "array-shared-duplicate",
            {
                let big = fixtures::big_object();
                Value::Array(vec![big.clone(), big])
            },
            false,
        ),
        // needs resolve (user ref marker)
        ("cycle", cyclic_fixture(), false),
    ]
}

#[test]
fn roundtrip_property_all_supported_types() {
    for (name, v, direct) in roundtrip_suite() {
        let bytes = encode(&v).unwrap_or_else(|e| panic!("{name}: encode failed: {e}"));
        assert_eq!(&bytes[..4], b"ODF1", "{name}: magic missing");
        let dec = decode(&bytes).unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));
        let resolved = resolve_refs(&dec).unwrap_or_else(|e| panic!("{name}: resolve failed: {e}"));
        assert!(
            bitexact_eq(&resolved, &v),
            "{name}: resolved roundtrip mismatch\n  original: {v:?}\n  resolved: {resolved:?}"
        );
        if direct {
            assert!(
                bitexact_eq(&dec, &v),
                "{name}: plain decode mismatch\n  original: {v:?}\n  decoded: {dec:?}"
            );
        }
        // deterministic: encoding twice is byte-identical
        assert_eq!(
            encode(&v).unwrap(),
            bytes,
            "{name}: encoder not deterministic"
        );
    }
}

// ---------- 2. structural sharing ----------

#[test]
fn structural_sharing_encodes_smaller_and_resolves_equal() {
    let big = fixtures::big_object();
    let mut big2 = big.clone();
    if let Value::Object(m) = &mut big2 {
        // same length, different content → must NOT be shared
        m.insert("f7".into(), Value::String("VALUE-CHANGED".into()));
    }
    let shared = encode(&Value::Array(vec![big.clone(), big.clone()])).unwrap();
    let distinct = encode(&Value::Array(vec![big.clone(), big2])).unwrap();
    assert!(
        shared.len() < distinct.len(),
        "shared payload ({} B) must be smaller than two inline copies ({} B)",
        shared.len(),
        distinct.len()
    );
    let dec = decode(&shared).unwrap();
    match &dec {
        Value::Array(a) => {
            assert!(
                matches!(a[0], Value::Object(_)),
                "first occurrence must be inline"
            );
            assert!(
                matches!(a[1], Value::Ref(1)),
                "second occurrence must be Ref(1), got {:?}",
                a[1]
            );
        }
        other => panic!("expected array, got {other:?}"),
    }
    let resolved = resolve_refs(&dec).unwrap();
    assert!(bitexact_eq(
        &resolved,
        &Value::Array(vec![big.clone(), big])
    ));
}

// ---------- 3. golden fixtures (stability guard) ----------

/// Regenerates the committed golden fixtures. Ignored by default so ordinary
/// test runs never mutate the working tree; run explicitly after a deliberate
/// wire-format change:
/// `cargo test -p oxide-odf generate_fixtures -- --ignored --nocapture`
/// (or `cargo run -p oxide-odf --example odf_fixture`).
#[test]
#[ignore = "writes fixture files; run explicitly with --ignored"]
fn generate_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    fixtures::write_all(dir).expect("writing golden fixtures");
    println!("wrote golden fixtures to {dir}");
}

#[test]
fn golden_fixtures_are_stable_and_decodable() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    for (name, value) in fixtures::golden() {
        let bin_path = format!("{dir}/{name}.odf.bin");
        let json_path = format!("{dir}/{name}.expected.json");
        let committed = std::fs::read(&bin_path)
            .unwrap_or_else(|e| panic!("missing golden fixture {bin_path} ({e}); run `cargo run -p oxide-odf --example odf_fixture`"));
        let fresh = encode(&value).unwrap_or_else(|e| panic!("{name}: encode failed: {e}"));
        assert_eq!(
            fresh, committed,
            "{name}: re-encoded bytes drifted from the committed fixture"
        );
        let expected_raw = std::fs::read_to_string(&json_path)
            .unwrap_or_else(|e| panic!("missing golden expectation {json_path} ({e})"));
        let expected: serde_json::Value =
            serde_json::from_str(&expected_raw).expect("expected.json parses");
        assert_eq!(
            expected,
            value.to_json(),
            "{name}: expected.json drifted from to_json output"
        );
        // decoder must accept every committed fixture
        decode(&committed)
            .unwrap_or_else(|e| panic!("{name}: committed fixture does not decode: {e}"));
    }
}

// ---------- 4. hardening (§17.1) ----------

#[test]
fn rejects_bad_magic() {
    assert_eq!(code_of(decode(b"")), "ODF-MAGIC");
    assert_eq!(code_of(decode(b"ODF")), "ODF-MAGIC");
    assert_eq!(code_of(decode(b"XDF1\x00")), "ODF-MAGIC");
    assert_eq!(code_of(decode(b"ODF2\x00")), "ODF-MAGIC");
    assert_eq!(code_of(decode(b"odf1\x00")), "ODF-MAGIC");
}

#[test]
fn rejects_truncated_input_at_every_prefix() {
    let value = fixtures::golden()
        .into_iter()
        .find(|(n, _)| *n == "kitchen-sink")
        .unwrap()
        .1;
    let payload = encode(&value).unwrap();
    assert!(payload.len() > 32);
    for len in 0..payload.len() {
        let r = decode(&payload[..len]);
        match r {
            Err(e) => assert!(
                e.0.code.starts_with("ODF-"),
                "prefix {len}: unexpected code {}",
                e.0.code
            ),
            Ok(_) => panic!("prefix of length {len} decoded successfully"),
        }
    }
    // trailing garbage after a complete root record is also rejected
    let mut trailing = payload.clone();
    trailing.push(0x00);
    assert_eq!(code_of(decode(&trailing)), "ODF-TRUNC");
}

#[test]
fn rejects_unknown_tags() {
    // magic + no shared entries + unknown tag 0x7F
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x7F);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // unknown typed-array kind byte
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x0D); // TYPED_ARRAY
    b.push(42); // bad kind
    b.push(0);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // typed array payload length not a multiple of the element size
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x0D); // TYPED_ARRAY
    b.push(8); // F64 → 8-byte elements
    push_varu(&mut b, 7);
    b.extend_from_slice(&[0u8; 7]);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // invalid UTF-8 in a string record
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x05); // STRING
    push_varu(&mut b, 2);
    b.extend_from_slice(&[0xFF, 0xFE]);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // malformed varuint (6 continuation bytes) — STRING lengths are varuints,
    // NUMBER is a fixed-width 8-byte record, so use a STRING to hit the path
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x05); // STRING
    b.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // invalid boolean byte
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x02); // BOOL
    b.push(2);
    assert_eq!(code_of(decode(&b)), "ODF-TAG");

    // invalid BigInt decimal string
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x04); // BIGINT
    push_varu(&mut b, 2);
    b.extend_from_slice(b"1a");
    assert_eq!(code_of(decode(&b)), "ODF-TAG");
}

#[test]
fn rejects_oversized_payloads() {
    let big = vec![0u8; MAX_BYTES + 1];
    assert_eq!(code_of(decode(&big)), "ODF-SIZE");
    // encode side: a payload that would exceed the limit is rejected too
    let huge_string = Value::String("x".repeat(MAX_BYTES));
    let r = encode(&Value::Array(vec![huge_string]));
    assert_eq!(code_of(r), "ODF-SIZE");
}

#[test]
fn rejects_depth_over_limit() {
    // encode side
    let build = |depth: u32| {
        let mut v = Value::Array(vec![]);
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    };
    assert!(
        encode(&build(MAX_DEPTH)).is_ok(),
        "depth == MAX_DEPTH must encode"
    );
    assert_eq!(code_of(encode(&build(MAX_DEPTH + 1))), "ODF-DEPTH");

    // decode side: crafted bytes
    let nested_bytes = |depth: u32| {
        let mut b = magic_buf();
        push_varu(&mut b, 0);
        for _ in 0..depth {
            b.push(0x09); // ARRAY
            push_varu(&mut b, 1);
        }
        b.push(0x09); // innermost empty ARRAY
        push_varu(&mut b, 0);
        b
    };
    let ok = decode(&nested_bytes(MAX_DEPTH - 1)).expect("depth == MAX_DEPTH must decode");
    assert!(matches!(ok, Value::Array(_)));
    assert_eq!(code_of(decode(&nested_bytes(MAX_DEPTH))), "ODF-DEPTH");
}

#[test]
fn rejects_invalid_refs() {
    // decode: Ref(0) with zero slots started
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x0F); // REF
    push_varu(&mut b, 0);
    assert_eq!(code_of(decode(&b)), "ODF-REF");

    // decode: forward ref beyond any started slot
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x09); // ARRAY
    push_varu(&mut b, 1);
    b.push(0x0F); // REF
    push_varu(&mut b, 99);
    b.push(0x00);
    assert_eq!(code_of(decode(&b)), "ODF-REF");

    // encode: hand-written refs pointing outside the table
    assert_eq!(code_of(encode(&Value::Ref(0))), "ODF-REF");
    assert_eq!(code_of(encode(&Value::Ref(99))), "ODF-REF");
    let with_containers = Value::Array(vec![Value::Object(BTreeMap::new()), Value::Ref(5)]);
    assert_eq!(code_of(encode(&with_containers)), "ODF-REF");
    // the cycle-cut marker Ref(u32::MAX) is a resolve-side concept, not encodable
    assert_eq!(code_of(encode(&Value::Ref(u32::MAX))), "ODF-REF");
}

#[test]
fn rejects_overfull_ref_tables() {
    // decode: root array with MAX_REFS children (root + children > MAX_REFS slots)
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x09); // ARRAY
    push_varu(&mut b, MAX_REFS);
    for _ in 0..MAX_REFS {
        b.push(0x09); // ARRAY
        push_varu(&mut b, 0);
    }
    assert_eq!(code_of(decode(&b)), "ODF-REF");
    // one child fewer stays exactly at the limit and decodes fine
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x09);
    push_varu(&mut b, MAX_REFS - 1);
    for _ in 0..MAX_REFS - 1 {
        b.push(0x09);
        push_varu(&mut b, 0);
    }
    assert!(
        decode(&b).is_ok(),
        "exactly MAX_REFS slots must be accepted"
    );
}

#[test]
fn rejects_prototype_polluting_keys() {
    // encode side (nested too)
    let mut m = BTreeMap::new();
    m.insert("__proto__".to_string(), Value::Object(BTreeMap::new()));
    assert_eq!(code_of(encode(&Value::Object(m))), "ODF-KEY");
    let mut inner = BTreeMap::new();
    inner.insert("constructor".to_string(), Value::Null);
    let nested = obj(&[("ok", Value::Object(inner))]);
    assert_eq!(code_of(encode(&nested)), "ODF-KEY");

    // decode side: craft an object with a "__proto__" key
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x0A); // OBJECT
    push_varu(&mut b, 1);
    push_varu(&mut b, 9);
    b.extend_from_slice(b"__proto__");
    b.push(0x00); // NULL
    assert_eq!(code_of(decode(&b)), "ODF-KEY");

    // "constructor" is rejected as well
    let mut b = magic_buf();
    push_varu(&mut b, 0);
    b.push(0x0A);
    push_varu(&mut b, 1);
    push_varu(&mut b, 11);
    b.extend_from_slice(b"constructor");
    b.push(0x00);
    assert_eq!(code_of(decode(&b)), "ODF-KEY");

    // map keys are data, not object structure → allowed
    let map = Value::Map(vec![(Value::String("__proto__".into()), Value::Null)]);
    let rt = resolve_refs(&decode(&encode(&map).unwrap()).unwrap()).unwrap();
    assert!(bitexact_eq(&rt, &map));
}

// ---------- 5. encode_with_shared ----------

#[test]
fn encode_with_shared_roundtrip() {
    let big = fixtures::big_object();
    let root = obj(&[
        ("a", Value::Ref(0)),
        ("b", Value::Ref(0)),
        ("c", Value::Number(1.0)),
    ]);
    let bytes = encode_with_shared(&root, &[(0, big.clone())]).unwrap();
    assert_eq!(&bytes[..4], b"ODF1");

    // plain decode returns the root with Ref markers intact
    let markers = decode(&bytes).unwrap();
    match &markers {
        Value::Object(m) => assert!(matches!(m.get("a"), Some(Value::Ref(0)))),
        other => panic!("expected object, got {other:?}"),
    }

    // full decode + resolve reconstructs the referenced value at every site
    let (rt, table) = decode_with_shared(&bytes).unwrap();
    assert_eq!(table.len(), 1);
    let resolved = resolve_refs_with(&rt, &table).unwrap();
    let expected = obj(&[
        ("a", big.clone()),
        ("b", big.clone()),
        ("c", Value::Number(1.0)),
    ]);
    assert!(bitexact_eq(&resolved, &expected));

    // the root auto-dedups against pre-registered shared entries
    let root2 = Value::Array(vec![big.clone(), big.clone()]);
    let bytes2 = encode_with_shared(&root2, &[(0, big.clone())]).unwrap();
    let (rt2, t2) = decode_with_shared(&bytes2).unwrap();
    let resolved2 = resolve_refs_with(&rt2, &t2).unwrap();
    assert!(bitexact_eq(
        &resolved2,
        &Value::Array(vec![big.clone(), big.clone()])
    ));

    // cycles among shared entries resolve with the back-edge left as a marker
    let e0 = obj(&[("other", Value::Ref(1))]);
    let e1 = obj(&[("other", Value::Ref(0))]);
    let bytes3 = encode_with_shared(&Value::Ref(0), &[(0, e0), (1, e1)]).unwrap();
    let (rt3, t3) = decode_with_shared(&bytes3).unwrap();
    let resolved3 = resolve_refs_with(&rt3, &t3).unwrap();
    let expected3 = obj(&[("other", obj(&[("other", Value::Ref(0))]))]);
    assert_eq!(resolved3, expected3);
}

#[test]
fn encode_with_shared_rejects_bad_tables() {
    let v = Value::Null;
    // duplicate index
    assert_eq!(
        code_of(encode_with_shared(&v, &[(0, v.clone()), (0, v.clone())])),
        "ODF-REF"
    );
    // sparse / out-of-range index (must be dense 0..n)
    assert_eq!(
        code_of(encode_with_shared(&v, &[(1, v.clone())])),
        "ODF-REF"
    );
    assert_eq!(
        code_of(encode_with_shared(&v, &[(0, v.clone()), (2, v.clone())])),
        "ODF-REF"
    );
    // refs from the root to a missing slot
    assert_eq!(
        code_of(encode_with_shared(&Value::Ref(3), &[(0, v.clone())])),
        "ODF-REF"
    );
}

// ---------- 6. ref resolution ----------

#[test]
fn resolve_refs_cycle_tolerant() {
    let v = cyclic_fixture();
    let dec = decode(&encode(&v).unwrap()).unwrap();
    let res = resolve_refs(&dec).unwrap();
    assert!(
        bitexact_eq(&res, &v),
        "cyclic roundtrip must reconstruct the original graph shape"
    );
    // the back-edge stays an explicit Ref marker
    let Value::Object(m) = &res else {
        panic!("expected object")
    };
    let Value::Object(peer) = m.get("peer").expect("peer") else {
        panic!("expected peer object")
    };
    assert_eq!(peer.get("back"), Some(&Value::Ref(0)));
}

#[test]
fn resolve_refs_depth_guard() {
    let build = |depth: u32| {
        let mut v = Value::Array(vec![Value::Number(1.0)]);
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    };
    assert!(
        resolve_refs(&build(MAX_DEPTH)).is_ok(),
        "MAX_DEPTH must resolve"
    );
    assert_eq!(code_of(resolve_refs(&build(MAX_DEPTH + 1))), "ODF-CYCLE");
}

#[test]
fn resolve_refs_rejects_unresolvable_and_keeps_cut_marker() {
    assert_eq!(code_of(resolve_refs(&Value::Ref(7))), "ODF-REF");
    // Ref(u32::MAX) is the documented cycle-cut marker: left intact
    assert_eq!(
        resolve_refs(&Value::Ref(u32::MAX)).unwrap(),
        Value::Ref(u32::MAX)
    );
    let nested = Value::Array(vec![Value::Ref(u32::MAX), Value::Null]);
    assert_eq!(resolve_refs(&nested).unwrap(), nested);
}

// ---------- 7. JSON interop ----------

#[test]
fn json_interop() {
    let j = json!({
        "s": "x", "n": 1.5, "b": true,
        "arr": [1, null, "two"],
        "nested": { "k": "v", "deep": { "z": 0 } }
    });
    let v = Value::from_json(&j);
    let bytes = encode(&v).unwrap();
    let rt = resolve_refs(&decode(&bytes).unwrap()).unwrap();
    assert_eq!(rt, v, "JSON → ODF → JSON roundtrip");
    assert_eq!(rt.to_json(), j);

    // mapping rules
    assert_eq!(Value::Undefined.to_json(), Json_ext_null());
    assert_eq!(
        Value::Date(1700000000000).to_json(),
        json!(1700000000000i64)
    );
    assert_eq!(Value::Number(f64::NAN).to_json(), Json_ext_null()); // lossy
    assert_eq!(Value::BigInt("123".into()).to_json(), json!("123"));
    assert_eq!(Value::Null.to_json(), Json_ext_null());

    // reserved keys survive from_json but are rejected at encode time
    let bad = Value::from_json(&json!({ "__proto__": { "x": 1 } }));
    assert_eq!(code_of(encode(&bad)), "ODF-KEY");
}

#[allow(non_snake_case)]
fn Json_ext_null() -> serde_json::Value {
    serde_json::Value::Null
}

// ---------- 8. encode_to_bytes / misc API ----------

#[test]
fn encode_to_bytes_feeds_decode() {
    let v = fixtures::golden()
        .into_iter()
        .find(|(n, _)| *n == "temporal")
        .unwrap()
        .1;
    let bytes = encode_to_bytes(&v).unwrap();
    let rt = resolve_refs(&decode(&bytes).unwrap()).unwrap();
    assert!(bitexact_eq(&rt, &v));
}

// ---------- 9. fuzz-ish robustness loop ----------

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn fuzz_decode_never_panics() {
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let golden = fixtures::golden();
    for i in 0..500u32 {
        let mut buf = match rng.next() % 3 {
            // pure garbage
            0 => {
                let len = (rng.next() % 300) as usize;
                (0..len)
                    .map(|_| (rng.next() & 0xFF) as u8)
                    .collect::<Vec<u8>>()
            }
            // garbage after a valid header
            1 => {
                let mut b = magic_buf();
                let len = (rng.next() % 120) as usize;
                b.extend((0..len).map(|_| (rng.next() & 0xFF) as u8));
                b
            }
            // bit-flipped valid encodings
            _ => {
                let (_, f) = &golden[(i as usize) % golden.len()];
                let mut b = encode(f).unwrap();
                let flips = 1 + (rng.next() % 8);
                for _ in 0..flips {
                    let idx = (rng.next() as usize) % b.len();
                    b[idx] ^= 1 << (rng.next() % 8);
                }
                b
            }
        };
        if rng.next() % 4 == 0 && buf.len() > 2 {
            buf.truncate((rng.next() as usize) % buf.len());
        }
        // must return Ok or Err — never panic, never hang
        let _ = decode(&buf);
    }
}
