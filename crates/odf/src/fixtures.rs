//! Golden fixture definitions shared by the fixture generator
//! (`examples/odf_fixture.rs`), the Rust stability test
//! (`tests/integration.rs`) and — via the committed bytes — the TS decoder
//! suite (`packages/core/test/odf.test.ts`, spec §9.5).
//!
//! `tests/fixtures/<name>.odf.bin` holds the encoded bytes and
//! `tests/fixtures/<name>.expected.json` the `Value::to_json()` output.

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
