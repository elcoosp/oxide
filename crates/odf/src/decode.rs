//! ODF decoder (spec §9.5) with §17.1 hardening: magic/version checks, depth
//! and size limits, bounded shared-slot tables, no prototype-polluting keys.
//!
//! `decode` returns the root value with `Value::Ref(i)` markers intact; the
//! virtual shared table (preamble entries followed by the root's inline
//! containers in pre-order) can be resolved with [`resolve_refs_with`]. For
//! payloads with a non-empty preamble use [`decode_with_shared`] to obtain
//! both parts.
//!
//! The read is iterative over an explicit frame stack (§17.1): a hostile
//! payload nested `MAX_DEPTH`-deep must fail with `ODF-DEPTH`, not by
//! overflowing the host thread's stack before the guard can fire.

use crate::wire::{Reader, tags};
use crate::{MAX_BYTES, MAX_DEPTH, MAX_REFS, Result, Value, err};
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
        shared.push(read_value(&mut r, n, &mut started)?);
    }

    let mut started: u32 = 0;
    let root = read_value(&mut r, n, &mut started)?;

    if !r.at_end() {
        return Err(err("ODF-TRUNC", "trailing bytes after the root value"));
    }
    Ok((root, shared))
}

/// A container under construction on the explicit work stack.
enum Frame {
    Array {
        items: Vec<Value>,
        remaining: u32,
    },
    Set {
        items: Vec<Value>,
        remaining: u32,
    },
    Object {
        map: BTreeMap<String, Value>,
        remaining: u32,
        pending_key: Option<String>,
    },
    Map {
        pairs: Vec<(Value, Value)>,
        remaining: u32,
        pending_key: Option<Value>,
    },
}

/// Read one root value iteratively. `base` is the number of preamble slots
/// (all of which count as started); `started` counts root-section containers
/// opened so far. A container record bumps `started` when the record begins,
/// so back-edges to in-progress ancestors validate — identical slot order to
/// the encoder's pre-order numbering.
fn read_value(r: &mut Reader<'_>, base: u32, started: &mut u32) -> Result<Value> {
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        // An object frame awaiting its next key reads a bare string (keys
        // carry no tag and consume no depth — same as the recursive reader).
        if let Some(Frame::Object {
            pending_key: None, ..
        }) = stack.last()
        {
            let key = r.string()?;
            if !crate::wire::valid_object_key(&key) {
                return Err(err(
                    "ODF-KEY",
                    format!("reserved object key `{key}` (§17.1: prototype pollution rejected)"),
                ));
            }
            if let Some(Frame::Object { pending_key, .. }) = stack.last_mut() {
                *pending_key = Some(key);
            }
            continue;
        }
        let depth = stack.len() as u32 + 1;
        if depth > MAX_DEPTH {
            return Err(err(
                "ODF-DEPTH",
                format!("value is nested deeper than the {MAX_DEPTH}-level limit"),
            ));
        }
        let tag = r.u8()?;
        // A complete value, unless the tag opens a container (pushed below).
        let value: Option<Value> = match tag {
            tags::NULL => Some(Value::Null),
            tags::UNDEFINED => Some(Value::Undefined),
            tags::BOOL => match r.u8()? {
                0 => Some(Value::Bool(false)),
                1 => Some(Value::Bool(true)),
                other => return Err(err("ODF-TAG", format!("invalid boolean byte {other}"))),
            },
            tags::NUMBER => Some(Value::Number(f64::from_bits(r.u64_le()?))),
            tags::BIGINT => {
                let s = r.string()?;
                if !crate::wire::validate_bigint(&s) {
                    return Err(err(
                        "ODF-TAG",
                        format!("invalid BigInt decimal string `{s}` (expected `-?(0|[0-9]+)`)"),
                    ));
                }
                Some(Value::BigInt(s))
            }
            tags::STRING => Some(Value::String(r.string()?)),
            tags::DATE => Some(Value::Date(r.i64_le()?)),
            tags::URL => Some(Value::Url(r.string()?)),
            tags::REGEXP => {
                let source = r.string()?;
                let flags = r.string()?;
                Some(Value::RegExp(source, flags))
            }
            tags::ARRAY => {
                open_slot(base, started)?;
                stack.push(Frame::Array {
                    items: Vec::new(),
                    remaining: r.varu()?,
                });
                None
            }
            tags::SET => {
                open_slot(base, started)?;
                stack.push(Frame::Set {
                    items: Vec::new(),
                    remaining: r.varu()?,
                });
                None
            }
            tags::OBJECT => {
                open_slot(base, started)?;
                stack.push(Frame::Object {
                    map: BTreeMap::new(),
                    remaining: r.varu()?,
                    pending_key: None,
                });
                None
            }
            tags::MAP => {
                open_slot(base, started)?;
                stack.push(Frame::Map {
                    pairs: Vec::new(),
                    remaining: r.varu()?,
                    pending_key: None,
                });
                None
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
                Some(Value::TypedArray(kind, r.take(len)?.to_vec()))
            }
            tags::ERROR => {
                let name = r.string()?;
                let message = r.string()?;
                let stack_flag = match r.u8()? {
                    0 => None,
                    1 => Some(r.string()?),
                    other => {
                        return Err(err("ODF-TAG", format!("invalid stack flag byte {other}")));
                    }
                };
                Some(Value::Error {
                    name,
                    message,
                    stack: stack_flag,
                })
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
                Some(Value::Ref(i))
            }
            tags::DEFERRED => Some(Value::Deferred { id: r.varu()? }),
            other => {
                return Err(err("ODF-TAG", format!("unknown type tag 0x{other:02X}")));
            }
        };
        // Immediately-completable empty containers still went through the
        // push above; fold them here so zero-length containers never linger.
        let mut value = value;
        if value.is_none() {
            match stack.last() {
                Some(Frame::Array { remaining: 0, .. })
                | Some(Frame::Set { remaining: 0, .. })
                | Some(Frame::Object { remaining: 0, .. })
                | Some(Frame::Map { remaining: 0, .. }) => {
                    value = Some(finish_frame(stack.pop().expect("just checked")));
                }
                _ => continue,
            }
        }
        // Place the finished value, collapsing completed ancestors.
        let mut v = value.expect("value or empty container");
        loop {
            match stack.last_mut() {
                None => return Ok(v),
                Some(frame) => match frame {
                    Frame::Array { items, remaining } => {
                        items.push(v);
                        *remaining -= 1;
                        if *remaining > 0 {
                            break;
                        }
                    }
                    Frame::Set { items, remaining } => {
                        items.push(v);
                        *remaining -= 1;
                        if *remaining > 0 {
                            break;
                        }
                    }
                    Frame::Object {
                        map,
                        remaining,
                        pending_key,
                    } => {
                        let key = pending_key.take().expect("object key precedes value");
                        map.insert(key, v);
                        *remaining -= 1;
                        if *remaining > 0 {
                            break;
                        }
                    }
                    Frame::Map {
                        pairs,
                        remaining,
                        pending_key,
                    } => {
                        if let Some(k) = pending_key.take() {
                            pairs.push((k, v));
                            *remaining -= 1;
                            if *remaining > 0 {
                                break;
                            }
                        } else {
                            // A finished map key: wait for its value.
                            *pending_key = Some(v);
                            break;
                        }
                    }
                },
            }
            // The top frame completed: pop, build, and place into its parent.
            v = finish_frame(stack.pop().expect("just matched"));
        }
    }
}

fn finish_frame(frame: Frame) -> Value {
    match frame {
        Frame::Array { items, .. } => Value::Array(items),
        Frame::Set { items, .. } => Value::Set(items),
        Frame::Object { map, .. } => Value::Object(map),
        Frame::Map { pairs, .. } => Value::Map(pairs),
    }
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
