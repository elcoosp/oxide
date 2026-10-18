//! Low-level wire primitives for the ODF format (spec §9.5): type tags, LEB128
//! varuints, the byte reader, bit-exact structural equality and the hardening
//! guards shared by the encoder and the decoder (§17.1).
//!
//! The full wire layout is documented in `crates/odf/docs/format.md`.

use crate::{Result, TypedArrayKind, Value, err};

/// Type-tag bytes. The byte equals the ordinal of the matching [Value] variant.
pub(crate) mod tags {
    pub const NULL: u8 = 0x00;
    pub const UNDEFINED: u8 = 0x01;
    pub const BOOL: u8 = 0x02;
    pub const NUMBER: u8 = 0x03;
    pub const BIGINT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const DATE: u8 = 0x06;
    pub const URL: u8 = 0x07;
    pub const REGEXP: u8 = 0x08;
    pub const ARRAY: u8 = 0x09;
    pub const OBJECT: u8 = 0x0A;
    pub const MAP: u8 = 0x0B;
    pub const SET: u8 = 0x0C;
    pub const TYPED_ARRAY: u8 = 0x0D;
    pub const ERROR: u8 = 0x0E;
    pub const REF: u8 = 0x0F;
    pub const DEFERRED: u8 = 0x10;
}

/// Reserved object keys (§17.1: no prototype pollution through ODF data).
pub(crate) fn valid_object_key(k: &str) -> bool {
    k != "__proto__" && k != "constructor"
}

/// A `BigInt` is stored as a decimal string, optionally negative (`-?(0|[0-9]+)`).
pub(crate) fn validate_bigint(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Map a wire kind byte onto a typed-array kind. The byte is the declaration
/// ordinal of [`TypedArrayKind`] (0 = I8 … 10 = BigUint64), little-endian
/// element order on the wire for all multi-byte kinds.
impl TypedArrayKind {
    /// Size of one element in bytes (spec §9.5 "typed arrays").
    pub fn element_size(self) -> usize {
        match self {
            TypedArrayKind::I8 | TypedArrayKind::U8 | TypedArrayKind::U8Clamped => 1,
            TypedArrayKind::I16 | TypedArrayKind::U16 => 2,
            TypedArrayKind::I32 | TypedArrayKind::U32 | TypedArrayKind::F32 => 4,
            TypedArrayKind::F64 | TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => 8,
        }
    }

    pub(crate) fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(TypedArrayKind::I8),
            1 => Some(TypedArrayKind::U8),
            2 => Some(TypedArrayKind::U8Clamped),
            3 => Some(TypedArrayKind::I16),
            4 => Some(TypedArrayKind::U16),
            5 => Some(TypedArrayKind::I32),
            6 => Some(TypedArrayKind::U32),
            7 => Some(TypedArrayKind::F32),
            8 => Some(TypedArrayKind::F64),
            9 => Some(TypedArrayKind::BigInt64),
            10 => Some(TypedArrayKind::BigUint64),
            _ => None,
        }
    }
}

/// Bit-exact structural equality between two values.
///
/// Unlike the derived `PartialEq` on [Value], this compares `f64` numbers by
/// their IEEE-754 bit pattern, so `NaN == NaN` (same payload) and `0.0 != -0.0`.
/// The encoder uses it for structural-sharing detection so that `-0.0` and
/// distinct NaN payloads are never merged.
///
/// Iterative over an explicit stack (§17.1): comparing hostile `MAX_DEPTH`-deep
/// values must not overflow the host thread's stack.
pub fn bitexact_eq(a: &Value, b: &Value) -> bool {
    let mut stack: Vec<(&Value, &Value)> = vec![(a, b)];
    while let Some((x, y)) = stack.pop() {
        match (x, y) {
            (Value::Null, Value::Null) => {}
            (Value::Undefined, Value::Undefined) => {}
            (Value::Bool(u), Value::Bool(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::Number(u), Value::Number(v)) => {
                if u.to_bits() != v.to_bits() {
                    return false;
                }
            }
            (Value::BigInt(u), Value::BigInt(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::String(u), Value::String(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::Date(u), Value::Date(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::Url(u), Value::Url(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::RegExp(s1, f1), Value::RegExp(s2, f2)) => {
                if s1 != s2 || f1 != f2 {
                    return false;
                }
            }
            (Value::Array(u), Value::Array(v)) | (Value::Set(u), Value::Set(v)) => {
                if u.len() != v.len() {
                    return false;
                }
                stack.extend(u.iter().zip(v.iter()));
            }
            (Value::Object(u), Value::Object(v)) => {
                if u.len() != v.len() {
                    return false;
                }
                for ((k1, v1), (k2, v2)) in u.iter().zip(v.iter()) {
                    if k1 != k2 {
                        return false;
                    }
                    stack.push((v1, v2));
                }
            }
            (Value::Map(u), Value::Map(v)) => {
                if u.len() != v.len() {
                    return false;
                }
                for ((k1, v1), (k2, v2)) in u.iter().zip(v.iter()) {
                    stack.push((k1, k2));
                    stack.push((v1, v2));
                }
            }
            (Value::TypedArray(k1, d1), Value::TypedArray(k2, d2)) => {
                if k1 != k2 || d1 != d2 {
                    return false;
                }
            }
            (
                Value::Error {
                    name: n1,
                    message: m1,
                    stack: s1,
                },
                Value::Error {
                    name: n2,
                    message: m2,
                    stack: s2,
                },
            ) => {
                if n1 != n2 || m1 != m2 || s1 != s2 {
                    return false;
                }
            }
            (Value::Ref(u), Value::Ref(v)) => {
                if u != v {
                    return false;
                }
            }
            (Value::Deferred { id: u }, Value::Deferred { id: v }) => {
                if u != v {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Append an unsigned LEB128 varuint (7 bits per byte, high bit = continuation).
pub(crate) fn write_varu(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Sequential reader over a payload slice. All multi-byte scalars are
/// little-endian; truncation is reported as `ODF-TRUNC`, malformed structure
/// as `ODF-TAG` (§17.1).
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.buf.len() - self.pos {
            return Err(err(
                "ODF-TRUNC",
                format!(
                    "payload ends prematurely: needed {n} more byte(s) at offset {}",
                    self.pos
                ),
            ));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u64_le(&mut self) -> Result<u64> {
        let mut b = [0u8; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }

    pub(crate) fn i64_le(&mut self) -> Result<i64> {
        Ok(self.u64_le()? as i64)
    }

    /// LEB128 varuint, at most 5 bytes for a `u32`. Overlong or overflowing
    /// encodings are rejected with `ODF-TAG`.
    pub(crate) fn varu(&mut self) -> Result<u32> {
        let mut result: u32 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift == 28 {
                if b & 0x80 != 0 {
                    return Err(err("ODF-TAG", "varuint encoding exceeds 5 bytes"));
                }
                if b > 0x0F {
                    return Err(err("ODF-TAG", "varuint overflows u32"));
                }
            }
            result |= ((b & 0x7F) as u32) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    /// A length-prefixed UTF-8 string (no tag byte). Invalid UTF-8 → `ODF-TAG`.
    pub(crate) fn string(&mut self) -> Result<String> {
        let len = self.varu()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| err("ODF-TAG", "string record is not valid UTF-8"))
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos == self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_varu(v: u32) {
        let mut buf = Vec::new();
        write_varu(&mut buf, v);
        let mut r = Reader::new(&buf);
        assert_eq!(r.varu().unwrap(), v, "varu roundtrip for {v}");
        assert!(r.at_end());
    }

    #[test]
    fn varu_roundtrips_boundary_values() {
        for v in [
            0u32,
            1,
            127,
            128,
            16_383,
            16_384,
            0x0FFF_FFFF,
            0x0FFF_FFFF + 1,
            u32::MAX,
        ] {
            roundtrip_varu(v);
        }
        // canonical width: u32::MAX takes exactly 5 bytes
        let mut buf = Vec::new();
        write_varu(&mut buf, u32::MAX);
        assert_eq!(buf.len(), 5);
    }

    #[test]
    fn varu_rejects_overlong_and_overflow() {
        // 6 continuation bytes
        let b = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert_eq!(Reader::new(&b).varu().unwrap_err().0.code, "ODF-TAG");
        // 5th byte carries bits above bit 31
        let b = [0xFF, 0xFF, 0xFF, 0xFF, 0x1F];
        assert_eq!(Reader::new(&b).varu().unwrap_err().0.code, "ODF-TAG");
        // EOF inside a varuint
        let b = [0x80];
        assert_eq!(Reader::new(&b).varu().unwrap_err().0.code, "ODF-TRUNC");
    }

    #[test]
    fn bitexact_numbers_distinguish_zero_sign_and_nan_payloads() {
        assert!(bitexact_eq(
            &Value::Number(f64::NAN),
            &Value::Number(f64::NAN)
        ));
        assert!(!bitexact_eq(&Value::Number(0.0), &Value::Number(-0.0)));
        assert!(!bitexact_eq(
            &Value::Number(f64::from_bits(0x7FF8_0000_0000_0001)),
            &Value::Number(f64::from_bits(0x7FF8_0000_0000_0002))
        ));
        // nested inside containers
        let a = Value::Array(vec![Value::Number(-0.0)]);
        let b = Value::Array(vec![Value::Number(0.0)]);
        assert!(!bitexact_eq(&a, &b));
        assert!(bitexact_eq(&a, &a));
        // different kinds are never equal
        assert!(!bitexact_eq(&Value::Null, &Value::Undefined));
        assert!(!bitexact_eq(&Value::Bool(true), &Value::Number(1.0)));
    }

    #[test]
    fn typed_array_kind_table() {
        let all = [
            TypedArrayKind::I8,
            TypedArrayKind::U8,
            TypedArrayKind::U8Clamped,
            TypedArrayKind::I16,
            TypedArrayKind::U16,
            TypedArrayKind::I32,
            TypedArrayKind::U32,
            TypedArrayKind::F32,
            TypedArrayKind::F64,
            TypedArrayKind::BigInt64,
            TypedArrayKind::BigUint64,
        ];
        for (i, k) in all.iter().enumerate() {
            assert_eq!(TypedArrayKind::from_byte(i as u8), Some(*k));
        }
        assert_eq!(TypedArrayKind::from_byte(11), None);
        assert_eq!(TypedArrayKind::U8.element_size(), 1);
        assert_eq!(TypedArrayKind::I16.element_size(), 2);
        assert_eq!(TypedArrayKind::F32.element_size(), 4);
        assert_eq!(TypedArrayKind::F64.element_size(), 8);
        assert_eq!(TypedArrayKind::BigUint64.element_size(), 8);
    }

    #[test]
    fn guards() {
        assert!(validate_bigint("0"));
        assert!(validate_bigint("-0"));
        assert!(validate_bigint("123456789012345678901234567890"));
        assert!(!validate_bigint(""));
        assert!(!validate_bigint("-"));
        assert!(!validate_bigint("1a"));
        assert!(!validate_bigint("+5"));
        assert!(!validate_bigint("1.5"));
        assert!(valid_object_key("anything"));
        assert!(!valid_object_key("__proto__"));
        assert!(!valid_object_key("constructor"));
    }
}
