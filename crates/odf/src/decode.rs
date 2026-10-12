//! ODF decoder (spec §9.5) with §17.1 hardening: magic/version checks, depth
//! and size limits, bounded shared-slot tables, no prototype-polluting keys.
//!
//! `decode` returns the root value with `Value::Ref(i)` markers intact; the
//! virtual shared table (preamble entries followed by the root's inline
//! containers in pre-order) can be resolved with [`resolve_refs_with`]. For
//! payloads with a non-empty preamble use [`decode_with_shared`] to obtain
//! both parts.

use crate::wire::{tags, Reader};
use crate::{err, Result, Value, MAX_BYTES, MAX_DEPTH, MAX_REFS};
use std::collections::BTreeMap;

/// Decode ODF bytes back into a [Value] plus its shared-value table.
///
/// The table holds the preamble entries (slots `0..n`); the root's inline
/// containers occupy the following slots in pre-order, exactly as the encoder
/// numbered them. Hardening per §17.1: `ODF-SIZE`, `ODF-MAGIC`, `ODF-DEPTH`,
/// `ODF-REF`, `ODF-TAG`, `ODF-KEY`, `ODF-TRUNC`.
pub fn decode_with_shared(bytes: &[u8]) -> Result<(Value, Vec<Value>)> {
    if bytes.len() > MAX_BYTES {
        return Err(err(
            "ODF-SIZE",
            format!("payload is {} bytes, limit is {MAX_BYTES}", bytes.len()),
        ));
    }
    // magic/version check first: short or mismatching prefixes are ODF-MAGIC,
    // never ODF-TRUNC (a 2-byte payload is not a truncated payload, it is garbage).
    if bytes.len() < crate::MAGIC.len() || bytes[..4] != crate::MAGIC {
        return Err(err(
            "ODF-MAGIC",
            "payload does not start with the b\"ODF1\" magic + version",
        ));
    }
    let mut r = Reader::new(bytes);
    r.take(4)?;
    let n = r.varu()?;
    if n as usize > MAX_REFS as usize {
        return Err(err(
            "ODF-REF",
            format!("shared table declares {n} entries, limit is {MAX_REFS}"),
        ));
    }

    let mut shared = Vec::new();
    for _ in 0..n {
        // entries are opaque: their internal containers occupy no slots,
        // and refs inside them may only target 0..n (base = n, started = 0).
        let mut started: u32 = 0;
        shared.push(read_value(&mut r, 1, n, &mut started)?);
    }

    let mut started: u32 = 0;
    let root = read_value(&mut r, 1, n, &mut started)?;

    if !r.at_end() {
        return Err(err("ODF-TRUNC", "trailing bytes after the root value"));
    }
    Ok((root, shared))
}

/// Read one record. `base` is the number of preamble slots (all of which count
/// as started); `started` counts root-section containers opened so far. A
/// container record bumps `started` when the record begins, so back-edges to
/// in-progress ancestors validate.
fn read_value(r: &mut Reader<'_>, depth: u32, base: u32, started: &mut u32) -> Result<Value> {
    if depth > MAX_DEPTH {
        return Err(err(
            "ODF-DEPTH",
            format!("value is nested deeper than the {MAX_DEPTH}-level limit"),
        ));
    }
    let tag = r.u8()?;
    Ok(match tag {
        tags::NULL => Value::Null,
        tags::UNDEFINED => Value::Undefined,
        tags::BOOL => match r.u8()? {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            other => return Err(err("ODF-TAG", format!("invalid boolean byte {other}"))),
        },
        tags::NUMBER => Value::Number(f64::from_bits(r.u64_le()?)),
        tags::BIGINT => {
            let s = r.string()?;
            if !crate::wire::validate_bigint(&s) {
                return Err(err(
                    "ODF-TAG",
                    format!("invalid BigInt decimal string `{s}` (expected `-?(0|[0-9]+)`)"),
                ));
            }
            Value::BigInt(s)
        }
        tags::STRING => Value::String(r.string()?),
        tags::DATE => Value::Date(r.i64_le()?),
        tags::URL => Value::Url(r.string()?),
        tags::REGEXP => {
            let source = r.string()?;
            let flags = r.string()?;
            Value::RegExp(source, flags)
        }
        tags::ARRAY => {
            open_slot(base, started)?;
            let count = r.varu()?;
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(read_value(r, depth + 1, base, started)?);
            }
            Value::Array(items)
        }
        tags::SET => {
            open_slot(base, started)?;
            let count = r.varu()?;
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(read_value(r, depth + 1, base, started)?);
            }
            Value::Set(items)
        }
        tags::OBJECT => {
            open_slot(base, started)?;
            let count = r.varu()?;
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let key = r.string()?;
                if !crate::wire::valid_object_key(&key) {
                    return Err(err(
                        "ODF-KEY",
                        format!(
                            "reserved object key `{key}` (§17.1: prototype pollution rejected)"
                        ),
                    ));
                }
                map.insert(key, read_value(r, depth + 1, base, started)?);
            }
            Value::Object(map)
        }
        tags::MAP => {
            open_slot(base, started)?;
            let count = r.varu()?;
            let mut pairs = Vec::new();
            for _ in 0..count {
                let key = read_value(r, depth + 1, base, started)?;
                let val = read_value(r, depth + 1, base, started)?;
                pairs.push((key, val));
            }
            Value::Map(pairs)
        }
        tags::TYPED_ARRAY => {
            let kb = r.u8()?;
            let kind = crate::TypedArrayKind::from_byte(kb).ok_or_else(|| {
                err(
                    "ODF-TAG",
                    format!("unknown typed-array kind byte 0x{kb:02X}"),
                )
            })?;
            let len = r.varu()? as usize;
            if len % kind.element_size() != 0 {
                return Err(err(
                    "ODF-TAG",
                    format!(
                        "typed array payload length {len} is not a multiple of the {}-byte element size",
                        kind.element_size()
                    ),
                ));
            }
            Value::TypedArray(kind, r.take(len)?.to_vec())
        }
        tags::ERROR => {
            let name = r.string()?;
            let message = r.string()?;
            let stack = match r.u8()? {
                0 => None,
                1 => Some(r.string()?),
                other => return Err(err("ODF-TAG", format!("invalid stack flag byte {other}"))),
            };
            Value::Error {
                name,
                message,
                stack,
            }
        }
        tags::REF => {
            let i = r.varu()?;
            if i as u64 >= base as u64 + *started as u64 {
                return Err(err(
                    "ODF-REF",
                    format!(
                        "Ref({i}) points outside the {} started slot(s)",
                        base + *started
                    ),
                ));
            }
            Value::Ref(i)
        }
        tags::DEFERRED => Value::Deferred { id: r.varu()? },
        other => {
            return Err(err("ODF-TAG", format!("unknown type tag 0x{other:02X}")));
        }
    })
}

/// Register a container slot and enforce the bounded-table limit (§17.1).
fn open_slot(base: u32, started: &mut u32) -> Result<()> {
    *started += 1;
    if base as u64 + *started as u64 > MAX_REFS as u64 {
        return Err(err(
            "ODF-REF",
            format!("more than {MAX_REFS} shared slots in one payload"),
        ));
    }
    Ok(())
}
