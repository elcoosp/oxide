#![doc = include_str!("../README.md")]

//! oxide-fw-odf — the Oxide Data Format (spec §9.5, D11).
//!
//! A devalue-compatible superset: JSON primitives plus `Date`, `Map`, `Set`, `BigInt`,
//! `URL`, `RegExp`, typed arrays, `undefined`, cyclic and shared references, `Error`,
//! and promises (as streamed deferred slots). The same bytes go to the cache and to the wire.
//! HTML embeds data in `<script type='application/oxide-data' nonce='...'>`.
//!
//! Wire layout (version byte + typed records, ref tables for shared/cyclic values):
//! see `docs/format.md`. The JS decoder (`packages/oxide`) is a ~1 KB module that
//! consumes the same bytes — golden fixtures keep both sides in lockstep.

use bytes::Bytes;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde_json::{Map as JsonMap, Number, Value as Json};
use std::collections::BTreeMap;

mod decode;
mod encode;
pub mod fixtures;
mod refs;
mod wire;

pub use decode::decode_with_shared;
pub use refs::{resolve_refs, resolve_refs_with};
pub use wire::bitexact_eq;

pub type Result<T> = std::result::Result<T, OxideError>;

/// Wire-format magic + version. First 4 bytes of every payload: `b"ODF1"`.
pub const MAGIC: [u8; 4] = *b"ODF1";

/// A resolved ODF value. Mirrors the spec §9.5 supported types.
/// Promises are represented at a higher layer as [Value::Deferred].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Undefined,
    Bool(bool),
    /// All finite numbers plus ±Infinity/NaN via f64 bit pattern.
    Number(f64),
    BigInt(String),
    String(String),
    /// `Date` — epoch milliseconds.
    Date(i64),
    /// `URL` — serialized string form.
    Url(String),
    /// `RegExp` — source + flags.
    RegExp(String, String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
    /// Preserves non-string keys ordering semantics of a JS Map.
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    /// Typed array kinds (spec §9.5 "typed arrays").
    TypedArray(TypedArrayKind, Vec<u8>),
    /// `Error` — name + message (+ stack).
    Error {
        name: String,
        message: String,
        stack: Option<String>,
    },
    /// A shared reference: index into the shared-value table (cycles + reuse).
    Ref(u32),
    /// A promise placeholder: resolved later by the streaming boundary protocol (§7.1).
    Deferred {
        id: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayKind {
    I8,
    U8,
    U8Clamped,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    BigInt64,
    BigUint64,
}

impl Value {
    /// Decode a JSON value into ODF (used for interop and fixtures).
    pub fn from_json(j: &Json) -> Value {
        match j {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            Json::String(s) => Value::String(s.clone()),
            Json::Array(a) => Value::Array(a.iter().map(Value::from_json).collect()),
            Json::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), Value::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// Lossy conversion to JSON (Date→ISO-ish epoch, BigInt→string, Ref→error at encode time instead).
    pub fn to_json(&self) -> Json {
        match self {
            Value::Null => Json::Null,
            Value::Undefined => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Number(n) => {
                // JSON has no int/float distinction, but serde_json does; keep
                // integral floats (e.g. 1.0) as JSON integers so a
                // from_json → encode → decode → to_json roundtrip is identity
                // for values that started as JSON integers. Anything outside
                // the exactly-representable integer range (or non-finite)
                // falls back to the f64 form, NaN/±Inf become `null` (lossy).
                if n.is_finite() && n.fract() == 0.0 && n.abs() <= 9.007_199_254_740_992e15 {
                    Json::Number(Number::from(*n as i64))
                } else {
                    Number::from_f64(*n).map(Json::Number).unwrap_or(Json::Null)
                }
            }
            Value::BigInt(s) => Json::String(s.clone()),
            Value::String(s) => Json::String(s.clone()),
            Value::Date(ms) => Json::Number(Number::from(*ms)),
            Value::Url(s) | Value::RegExp(s, _) => Json::String(s.clone()),
            Value::Array(a) => Json::Array(a.iter().map(|v| v.to_json()).collect()),
            Value::Object(o) => {
                let mut m = JsonMap::new();
                for (k, v) in o {
                    m.insert(k.clone(), v.to_json());
                }
                Json::Object(m)
            }
            Value::Map(pairs) => {
                let mut m = JsonMap::new();
                for (k, v) in pairs {
                    let key = match k {
                        Value::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    m.insert(key, v.to_json());
                }
                Json::Object(m)
            }
            Value::Set(items) => Json::Array(items.iter().map(|v| v.to_json()).collect()),
            Value::TypedArray(_, data) => Json::Array(
                data.iter()
                    .map(|b| Json::Number(Number::from(*b as u64)))
                    .collect(),
            ),
            Value::Error { name, message, .. } => {
                let mut m = JsonMap::new();
                m.insert("name".into(), Json::String(name.clone()));
                m.insert("message".into(), Json::String(message.clone()));
                Json::Object(m)
            }
            Value::Ref(_) => Json::Null,
            Value::Deferred { id } => {
                let mut m = JsonMap::new();
                m.insert("__deferred".into(), Json::Number(Number::from(*id)));
                Json::Object(m)
            }
        }
    }
}

/// Encode a value to ODF bytes. Shared/cyclic references are detected automatically
/// (repeated object/array identity becomes a `Ref`). `Value::Ref` values constructed
/// by hand with invalid indices fail with `ODF-REF`.
pub fn encode(value: &Value) -> Result<Vec<u8>> {
    encode::encode_with_preamble(value, &[])
}

/// Encode into a pooled `Bytes` buffer (cache feeds on bytes directly, §9.5).
pub fn encode_to_bytes(value: &Value) -> Result<Bytes> {
    Ok(Bytes::from(encode(value)?))
}

/// Decode ODF bytes back into a [Value]. Hardening per §17.1: depth limit,
/// size limits and bounded reference tables; no prototype-polluting keys survive.
///
/// `Value::Ref(i)` markers are returned intact; resolve them with
/// [resolve_refs] (plain payloads) or [resolve_refs_with] +
/// [decode_with_shared] (payloads carrying a shared preamble).
pub fn decode(bytes: &[u8]) -> Result<Value> {
    Ok(decode::decode_with_shared(bytes)?.0)
}

/// Encode with a pre-resolved shared table (used by the render pipeline when the
/// same bytes feed both the cache and an inline `<script type='application/oxide-data'>`).
///
/// The table must be dense: indices `0..shared.len()`, no duplicates; each entry
/// becomes slot `i`, and `Value::Ref(i)` markers inside `value` point at it.
/// Refs are also detected automatically against the shared entries.
pub fn encode_with_shared(value: &Value, shared: &[(u32, Value)]) -> Result<Vec<u8>> {
    encode::encode_with_preamble(value, shared)
}

/// Maximum nesting depth accepted by `decode` (§17.1 ODF decoder hardening).
pub const MAX_DEPTH: u32 = 512;
/// Maximum payload size accepted by `decode` (§17.1).
pub const MAX_BYTES: usize = 256 * 1024 * 1024;
/// Maximum entries in the shared-reference table (§17.1 bounded ref tables).
pub const MAX_REFS: u32 = 1 << 20;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// Build a two-node cyclic structure for tests and golden fixtures:
/// `a = { name: "a", peer: b }`, `b = { name: "b", back: Ref(0) }` where
/// slot 0 is `a` itself. Encoding writes the back-edge as `Ref(0)`;
/// [resolve_refs] keeps the back-edge marker so the cycle stays visible.
pub fn cyclic_fixture() -> Value {
    let mut b = BTreeMap::new();
    b.insert("name".to_string(), Value::String("b".to_string()));
    b.insert("back".to_string(), Value::Ref(0));
    let mut a = BTreeMap::new();
    a.insert("name".to_string(), Value::String("a".to_string()));
    a.insert("peer".to_string(), Value::Object(b));
    Value::Object(a)
}
