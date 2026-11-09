//! Golden fixture definitions shared by the fixture generator
//! (`examples/odf_fixture.rs`), the Rust stability test
//! (`tests/integration.rs`) and — via the committed bytes — the TS decoder
//! suite (`packages/core/test/odf.test.ts`, spec §9.5).
//!
//! `tests/fixtures/<name>.odf.bin` holds the encoded bytes and
//! `tests/fixtures/<name>.expected.json` the `Value::to_json()` output.
//!
//! The F06 patch goldens (`patch_golden`, spec §V2-F06 / D30) live in the
//! `tests/fixtures/patch/` subdirectory — `patch_<name>.base.odf.bin` (the
//! full ODF base bytes), `patch_<name>.odp.bin` (the ODP1 patch bytes) and
//! `patch_<name>.expected.json` (`Value::to_json()` of the applied value, or
//! `{"error":"ODP-BASE"}` for the mismatch case). They are consumed by both
//! the Rust suite (`tests/patch_tdd.rs`) and the TS suite
//! (`packages/core/test/odf-patch.test.ts`).

use crate::TypedArrayKind as K;
use crate::Value;
use std::collections::BTreeMap;

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn arr(items: &[Value]) -> Value {
    Value::Array(items.to_vec())
}

fn ta(kind: K, bytes: &[u8]) -> Value {
    Value::TypedArray(kind, bytes.to_vec())
}

fn le_i16(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_u16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le_i32(v: &[i32]) -> Vec<u8> {
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

/// A mid-sized object used by the structural-sharing tests and fixture.
pub fn big_object() -> Value {
    obj(&[
        ("f0", Value::String("value-0".into())),
        ("f1", Value::String("value-1".into())),
        ("f2", Value::String("value-2".into())),
        ("f3", Value::String("value-3".into())),
        ("f4", Value::Number(4.5)),
        ("f5", Value::Date(1700000000000)),
        (
            "f6",
            arr(&[Value::Number(1.0), Value::Bool(false), Value::Null]),
        ),
        ("f7", Value::String("value-7".into())),
    ])
}

/// The golden fixture set: name → value. Order is stable; keep names in sync
/// with the committed files under `tests/fixtures/`.
pub fn golden() -> Vec<(&'static str, Value)> {
    vec![
        (
            "primitives",
            obj(&[
                ("null", Value::Null),
                ("undefined", Value::Undefined),
                ("true", Value::Bool(true)),
                ("false", Value::Bool(false)),
                ("int", Value::Number(42.0)),
                ("float", Value::Number(-273.15)),
            ]),
        ),
        (
            "number-specials",
            arr(&[
                Value::Number(0.0),
                Value::Number(-0.0),
                Value::Number(f64::NAN),
                Value::Number(f64::INFINITY),
                Value::Number(f64::NEG_INFINITY),
                Value::Number(5e-324),
                Value::Number(1.7976931348623157e308),
            ]),
        ),
        (
            "strings",
            arr(&[
                Value::String(String::new()),
                Value::String("héllo 🌍 日本語 ça va".into()),
                Value::String("line\nbreak \"quoted\" \\back\\\ttab".into()),
                Value::String("\u{0}\u{1}\u{1f}ctrl".into()),
            ]),
        ),
        (
            "bigint",
            arr(&[
                Value::BigInt("0".into()),
                Value::BigInt(
                    "123456789012345678901234567890123456789012345678901234567890".into(),
                ),
                Value::BigInt("-98765432109876543210987654321098765432109876543210".into()),
            ]),
        ),
        (
            "temporal",
            obj(&[
                ("date0", Value::Date(0)),
                ("date-neg", Value::Date(-62135596800000)),
                ("date-max", Value::Date(i64::MAX)),
                (
                    "url",
                    Value::Url("https://oxide.rs/docs?a=1&b=2#frag".into()),
                ),
                ("regexp", Value::RegExp("^a.*b$\\d".into(), "gimsu".into())),
            ]),
        ),
        (
            "empty-containers",
            arr(&[
                Value::Array(vec![]),
                Value::Object(BTreeMap::new()),
                Value::Map(vec![]),
                Value::Set(vec![]),
            ]),
        ),
        (
            "nested-arrays",
            arr(&[
                arr(&[
                    Value::Number(1.0),
                    arr(&[Value::Number(2.0), arr(&[Value::Number(3.0)])]),
                ]),
                Value::String("x".into()),
            ]),
        ),
        (
            "object-keys-sorted",
            obj(&[
                ("z", Value::Number(1.0)),
                ("a", Value::Number(2.0)),
                ("Ä", Value::Number(3.0)),
                ("0", Value::Number(4.0)),
            ]),
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
        ),
        (
            "typed-arrays",
            obj(&[
                ("i8", ta(K::I8, &[255, 0, 127])),
                ("u8", ta(K::U8, &[0, 1, 254, 255])),
                ("u8clamped", ta(K::U8Clamped, &[0, 128, 255])),
                ("i16", ta(K::I16, &le_i16(&[-2, -1, 0, 1, 2]))),
                ("u16", ta(K::U16, &le_u16(&[0xFFFF, 1]))),
                ("i32", ta(K::I32, &le_i32(&[-2, 0, 2]))),
                ("u32", ta(K::U32, &le_u32(&[0xFFFF_FFFF, 1]))),
                ("f32", ta(K::F32, &le_f32(&[1.5, -0.25, f32::INFINITY]))),
                ("f64", ta(K::F64, &le_f64(&[1.5, -2.25, f64::NAN, -0.0]))),
                ("bigint64", ta(K::BigInt64, &le_i64(&[i64::MIN, i64::MAX]))),
                ("biguint64", ta(K::BigUint64, &le_u64(&[u64::MAX, 1]))),
            ]),
        ),
        (
            "errors",
            arr(&[
                Value::Error {
                    name: "TypeError".into(),
                    message: "boom".into(),
                    stack: None,
                },
                Value::Error {
                    name: "RangeError".into(),
                    message: "out of range".into(),
                    stack: Some("at f (x.ts:1:1)".into()),
                },
            ]),
        ),
        (
            "deferred",
            obj(&[
                ("a", Value::Deferred { id: 1 }),
                ("b", Value::Deferred { id: 2 }),
            ]),
        ),
        ("shared-structure", {
            let big = big_object();
            Value::Array(vec![big.clone(), big])
        }),
        ("cycle", crate::cyclic_fixture()),
        (
            "kitchen-sink",
            obj(&[
                (
                    "scalars",
                    arr(&[
                        Value::Null,
                        Value::Undefined,
                        Value::Bool(true),
                        Value::Number(-0.0),
                        Value::Number(f64::NAN),
                        Value::BigInt("42".into()),
                    ]),
                ),
                ("when", Value::Date(1234567890123)),
                ("where", Value::Url("https://oxide.rs/?q=≥".into())),
                ("re", Value::RegExp("(a|b)+".into(), "gi".into())),
                (
                    "err",
                    Value::Error {
                        name: "EvalError".into(),
                        message: "no".into(),
                        stack: None,
                    },
                ),
                ("later", Value::Deferred { id: 9 }),
                (
                    "map",
                    Value::Map(vec![
                        (Value::Date(1), Value::String("one".into())),
                        (Value::String("two".into()), Value::Number(2.0)),
                    ]),
                ),
                (
                    "set",
                    Value::Set(vec![
                        Value::Null,
                        Value::Number(1.0),
                        Value::String("1".into()),
                    ]),
                ),
                ("u8s", ta(K::U8, &[1, 2, 3])),
                (
                    "deep",
                    arr(&[obj(&[("inner", arr(&[Value::Array(vec![])]))])]),
                ),
            ]),
        ),
    ]
}

/// Write every golden fixture into `dir` as `<name>.odf.bin` (encoded bytes)
/// and `<name>.expected.json` ([`Value::to_json`] of the decoded value).
///
/// Called by the `odf_fixture` example; the output must be byte-stable across
/// releases because the TS decoder suite consumes the committed files.
pub fn write_all(dir: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, value) in golden() {
        let bytes = crate::encode(&value).expect("golden fixture must encode cleanly");
        std::fs::write(format!("{dir}/{name}.odf.bin"), &bytes)?;
        std::fs::write(
            format!("{dir}/{name}.expected.json"),
            value.to_json().to_string(),
        )?;
    }
    Ok(())
}

// ------------------------------------------------------------- F06 patches

/// One committed F06 patch golden case (D30's 12-case list).
pub struct PatchGolden {
    /// Fixture stem: `patch_<name>.{base.odf.bin,odp.bin,expected.json}`.
    pub name: &'static str,
    /// The full snapshot the patch chains from.
    pub base: Value,
    /// The next snapshot: `ops = diff(base, new)` at generation time.
    pub new: Value,
    /// `true` for the `base-mismatch` case: the committed patch carries a
    /// deliberately wrong base id, and both implementations must refuse to
    /// apply it (the reconnect trigger, not an applied value).
    pub expect_mismatch: bool,
}

/// The 12 spec-named patch cases (§V2-F06: "set, nested set, delete, append,
/// prepend, middle splice, truncate, type change, Map untouched, deep array
/// of objects, no-op, base mismatch"). Order is stable; keep names in sync
/// with the committed files under `tests/fixtures/patch/`.
pub fn patch_golden() -> Vec<PatchGolden> {
    let mut rows: Vec<Value> = (0..6)
        .map(|k| {
            Value::Object(
                [
                    ("i".to_string(), Value::Number(k as f64)),
                    ("name".to_string(), Value::String(format!("row-{k}"))),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    rows[3] = Value::Object(
        [
            ("i".to_string(), Value::Number(3.0)),
            (
                "name".to_string(),
                Value::String("row-3-renamed".to_string()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    vec![
        PatchGolden {
            name: "set",
            base: Value::Object(
                [
                    ("a".into(), Value::Number(1.0)),
                    ("keep".into(), Value::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ),
            new: Value::Object(
                [
                    ("a".into(), Value::Number(2.0)),
                    ("keep".into(), Value::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "nested-set",
            base: Value::Object(
                [(
                    "deep".into(),
                    Value::Object(
                        [(
                            "x".into(),
                            Value::Object([("c".into(), Value::Number(1.0))].into_iter().collect()),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                )]
                .into_iter()
                .collect(),
            ),
            // The new leaf is a Date: rich values ride inside Set ops.
            new: Value::Object(
                [(
                    "deep".into(),
                    Value::Object(
                        [(
                            "x".into(),
                            Value::Object(
                                [("c".into(), Value::Date(1_234_567_890_123))]
                                    .into_iter()
                                    .collect(),
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                )]
                .into_iter()
                .collect(),
            ),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "delete",
            base: Value::Object(
                [
                    ("a".into(), Value::Number(1.0)),
                    ("gone".into(), Value::String("bye".into())),
                ]
                .into_iter()
                .collect(),
            ),
            new: Value::Object([("a".into(), Value::Number(1.0))].into_iter().collect()),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "append",
            base: Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]),
            new: Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
            ]),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "prepend",
            base: Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]),
            new: Value::Array(vec![
                Value::Number(0.0),
                Value::Number(1.0),
                Value::Number(2.0),
            ]),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "middle-splice",
            base: Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]),
            new: Value::Array(vec![
                Value::Number(1.0),
                Value::Number(9.0),
                Value::Number(3.0),
            ]),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "truncate",
            base: Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
            ]),
            new: Value::Array(vec![Value::Number(1.0)]),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "type-change",
            base: Value::Object(
                [(
                    "v".into(),
                    Value::Object([("k".into(), Value::Number(1.0))].into_iter().collect()),
                )]
                .into_iter()
                .collect(),
            ),
            new: Value::Object(
                [("v".into(), Value::Array(vec![Value::Number(1.0)]))]
                    .into_iter()
                    .collect(),
            ),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "map-untouched",
            base: Value::Object(
                [
                    (
                        "m".into(),
                        Value::Map(vec![(Value::String("k".into()), Value::Number(1.0))]),
                    ),
                    ("x".into(), Value::Number(1.0)),
                ]
                .into_iter()
                .collect(),
            ),
            new: Value::Object(
                [
                    (
                        "m".into(),
                        Value::Map(vec![(Value::String("k".into()), Value::Number(1.0))]),
                    ),
                    ("x".into(), Value::Number(2.0)),
                ]
                .into_iter()
                .collect(),
            ),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "deep-array-of-objects",
            base: Value::Object(
                [("rows".into(), Value::Array(rows.clone()))]
                    .into_iter()
                    .collect(),
            ),
            new: Value::Object([("rows".into(), Value::Array(rows))].into_iter().collect()),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "no-op",
            base: Value::Object(
                [
                    ("a".into(), Value::Number(1.0)),
                    ("b".into(), Value::String("x".into())),
                ]
                .into_iter()
                .collect(),
            ),
            new: Value::Object(
                [
                    ("a".into(), Value::Number(1.0)),
                    ("b".into(), Value::String("x".into())),
                ]
                .into_iter()
                .collect(),
            ),
            expect_mismatch: false,
        },
        PatchGolden {
            name: "base-mismatch",
            base: Value::Object([("a".into(), Value::Number(1.0))].into_iter().collect()),
            new: Value::Object([("a".into(), Value::Number(2.0))].into_iter().collect()),
            expect_mismatch: true,
        },
    ]
}

/// Write every patch golden into `dir` (the `tests/fixtures/patch`
/// subdirectory): `<stem>.base.odf.bin`, `<stem>.odp.bin` and
/// `<stem>.expected.json`. Byte-stable across releases (the TS patch suite
/// consumes the committed files).
///
/// # Panics
///
/// Panics if a base or patch fails to encode (fixtures are compile-time
/// constants; a failure is a bug in `patch`, not bad input).
pub fn write_patch_all(dir: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for case in patch_golden() {
        let base_bytes = crate::encode(&case.base).expect("patch golden base must encode cleanly");
        let ops = crate::patch::diff(&case.base, &case.new);
        let applied =
            crate::patch::apply(&case.base, &ops).expect("patch golden ops must apply cleanly");
        // The base id hashes the FULL ODF bytes of the base (D30). The
        // mismatch case deliberately carries a wrong 16-hex id.
        let b = if case.expect_mismatch {
            "deadbeefdeadbeef".to_string()
        } else {
            crate::patch::base_id(&base_bytes)
        };
        let patch_bytes =
            crate::patch::encode_patch(&b, &ops).expect("patch golden ops must encode cleanly");
        let expected = if case.expect_mismatch {
            serde_json::json!({ "error": "ODP-BASE" }).to_string()
        } else {
            applied.to_json().to_string()
        };
        std::fs::write(
            format!("{dir}/patch_{}.base.odf.bin", case.name),
            &base_bytes,
        )?;
        std::fs::write(format!("{dir}/patch_{}.odp.bin", case.name), &patch_bytes)?;
        std::fs::write(format!("{dir}/patch_{}.expected.json", case.name), expected)?;
    }
    Ok(())
}
