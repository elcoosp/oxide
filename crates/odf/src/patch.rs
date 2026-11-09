//! The F06 ODF patch substrate (spec v0.2 §V2-F06, decision refinement D30).
//!
//! A patch is a distinct payload with the outer magic `ODP1` followed by an
//! ordinary [`ODF1`](crate::MAGIC) payload whose root value is
//! `{ b: <base id>, o: [ops] }` — the existing encoder and decoder are reused
//! byte for byte; only the outer 4-byte prefix is new. `b` is the 16-hex
//! truncated SHA-256 of the **full ODF bytes of the base snapshot** (the
//! client hashes the bytes it last decoded; the server computes the same
//! truncation with `sha2` — deliberately not blake3, so the tiny client has
//! a one-algorithm path).
//!
//! The structural diff follows the spec verbatim: objects recurse by key;
//! arrays use common-prefix and common-suffix trimming, so append, prepend,
//! middle-replace and truncate each become one [`Op::Splice`]; every other
//! change is a whole-value [`Op::Set`]. Maps, Sets, typed arrays, Errors and
//! Dates are opaque to the diff — an unequal one is a whole `Set`, an equal
//! one emits nothing.
//!
//! Deliberate deltas from the spec text (recorded in `docs/deviations.md`):
//! - `apply`/`decode_patch` return [`crate::Result`] with `ODP-*` diagnostic
//!   codes instead of bare `String` errors.
//! - Reserved keys (`__proto__`, `constructor`) are rejected at **decode**
//!   time (`ODP-KEY`) in addition to the spec's TS-side apply-time rejection:
//!   the Rust encoder already refuses them as object keys, so a patch that
//!   writes one is malformed at the parse boundary, not mid-apply.
//! - Wire integers (path indices, splice bounds) are f64 on the wire but
//!   parsed through the exact-integer range (≤ 2^53) into `usize` — cast-free
//!   and overflow-safe on 32-bit targets too.
//!
//! The sending policy itself (patch only when `patch.len() * 2 < full.len()`
//! and `ops.len() <= PATCH_MAX_OPS`) lives with the transport — see
//! `oxide_fw_server::live::delta_payload`.

use crate::Value;
use std::collections::BTreeMap;

/// Patch wire magic: the outer 4-byte prefix of every patch payload.
pub const MAGIC_PATCH: [u8; 4] = *b"ODP1";

/// The D30 op-count bound: a rerun whose diff produces more than this many
/// ops is shipped as a full snapshot instead of a patch.
pub const PATCH_MAX_OPS: usize = 64;

/// The exact-integer ceiling for wire numbers that become indices/bounds
/// (2^53 — beyond it f64 loses integer precision).
const MAX_WIRE_INT: f64 = 9_007_199_254_740_992.0;

// ------------------------------------------------------------------- model

/// One path segment: an object key or an array index.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Key(String),
    Idx(usize),
}

/// One structural operation ([`diff`] produces them, [`apply`] consumes
/// them, `ops_to_value`/`value_to_ops` put them on the wire).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Replace (or create) the value at `path`.
    Set { path: Vec<Seg>, value: Value },
    /// Remove the object key at `path` (the last segment must be a
    /// [`Seg::Key`]; arrays shrink through [`Op::Splice`] instead).
    Del { path: Vec<Seg> },
    /// Replace `remove` elements of the array at `path` starting at `at`
    /// with `insert`.
    Splice {
        path: Vec<Seg>,
        at: usize,
        remove: usize,
        insert: Vec<Value>,
    },
}

// ------------------------------------------------------------------- diff

/// Structural diff between two snapshots: the ops that turn `old` into
/// `new`. Emits whole-value `Set`s for every non-object/array change — the
/// caller-sized cost of a patch is bounded by the sending policy, not here.
#[must_use]
pub fn diff(old: &Value, new: &Value) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut path = Vec::new();
    walk(&mut path, old, new, &mut ops);
    ops
}

fn walk(path: &mut Vec<Seg>, old: &Value, new: &Value, out: &mut Vec<Op>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                path.push(Seg::Key(k.clone()));
                match a.get(k) {
                    Some(ov) => walk(path, ov, v, out),
                    None => out.push(Op::Set {
                        path: path.clone(),
                        value: v.clone(),
                    }),
                }
                path.pop();
            }
            for k in a.keys() {
                if !b.contains_key(k) {
                    let mut p = path.clone();
                    p.push(Seg::Key(k.clone()));
                    out.push(Op::Del { path: p });
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let pre = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
            let suf = a[pre..]
                .iter()
                .rev()
                .zip(b[pre..].iter().rev())
                .take_while(|(x, y)| x == y)
                .count();
            let (ra, rb) = (a.len() - pre - suf, b.len() - pre - suf);
            if ra > 0 || rb > 0 {
                out.push(Op::Splice {
                    path: path.clone(),
                    at: pre,
                    remove: ra,
                    insert: b[pre..pre + rb].to_vec(),
                });
            }
        }
        _ => out.push(Op::Set {
            path: path.clone(),
            value: new.clone(),
        }),
    }
}

// ------------------------------------------------------------------- apply

/// Apply `ops` to a snapshot, producing the new value. The base is never
/// mutated (the ops carry owned values; the result is a fresh tree).
///
/// # Errors
///
/// - `ODP-KEY`: a reserved key (`__proto__`/`constructor`) appears in a
///   path (prototype-pollution hardening, same rule as the encoder).
/// - `ODP-PATH`: the path does not match the base's shape (missing
///   intermediate key, out-of-bounds index, walk through a Map/Set/…).
/// - `ODP-DEL`: a `Del` targets anything other than an object key.
/// - `ODP-SPLICE`: the splice target is not an array, or `at + remove`
///   runs past its end.
pub fn apply(base: &Value, ops: &[Op]) -> crate::Result<Value> {
    let mut v = base.clone();
    for op in ops {
        match op {
            Op::Set { path, value } => {
                check_path(path)?;
                let slot = slot(&mut v, path, true)?;
                *slot = value.clone();
            }
            Op::Del { path } => {
                check_path(path)?;
                let Some((last, parent)) = path.split_last() else {
                    return Err(crate::err("ODP-PATH", "del path is empty"));
                };
                let target = slot(&mut v, parent, false)?;
                match (target, last) {
                    (Value::Object(m), Seg::Key(k)) => {
                        m.remove(k);
                    }
                    _ => return Err(crate::err("ODP-DEL", "del target is not an object key")),
                }
            }
            Op::Splice {
                path,
                at,
                remove,
                insert,
            } => {
                check_path(path)?;
                let target = slot(&mut v, path, false)?;
                let Value::Array(a) = target else {
                    return Err(crate::err("ODP-SPLICE", "splice target is not an array"));
                };
                let end = at
                    .checked_add(*remove)
                    .ok_or_else(|| crate::err("ODP-SPLICE", "splice range overflows usize"))?;
                if end > a.len() {
                    return Err(crate::err(
                        "ODP-SPLICE",
                        format!(
                            "splice range {at}..{end} exceeds the array length {}",
                            a.len()
                        ),
                    ));
                }
                a.splice(*at..end, insert.iter().cloned());
            }
        }
    }
    Ok(v)
}

/// Reserved-key rejection for every path segment (write-side guard; the
/// decode side rejects earlier in `path_from_wire`).
fn check_path(path: &[Seg]) -> crate::Result<()> {
    for seg in path {
        if let Seg::Key(k) = seg
            && !crate::wire::valid_object_key(k)
        {
            return Err(crate::err(
                "ODP-KEY",
                format!("reserved key {k:?} cannot appear in a patch path"),
            ));
        }
    }
    Ok(())
}

/// Walk `path` from `v` (the spec's `slot`). With `create`, a missing **leaf
/// key** is inserted as `Null` first; missing intermediates always error.
fn slot<'a>(v: &'a mut Value, path: &[Seg], create: bool) -> crate::Result<&'a mut Value> {
    let mut cur = v;
    let last = path.len().saturating_sub(1);
    for (i, seg) in path.iter().enumerate() {
        cur = match (cur, seg) {
            (Value::Object(m), Seg::Key(k)) => {
                if create && i == last && !m.contains_key(k) {
                    m.insert(k.clone(), Value::Null);
                }
                m.get_mut(k)
                    .ok_or_else(|| crate::err("ODP-PATH", format!("missing key {k:?}")))?
            }
            (Value::Object(_), Seg::Idx(n)) => {
                return Err(crate::err(
                    "ODP-PATH",
                    format!("index {n} cannot walk an object"),
                ));
            }
            (Value::Array(a), Seg::Idx(n)) => a
                .get_mut(*n)
                .ok_or_else(|| crate::err("ODP-PATH", format!("missing index {n}")))?,
            (Value::Array(_), Seg::Key(k)) => {
                return Err(crate::err(
                    "ODP-PATH",
                    format!("key {k:?} cannot walk an array"),
                ));
            }
            _ => return Err(crate::err("ODP-PATH", "path does not match value shape")),
        };
    }
    Ok(cur)
}

// ------------------------------------------------------------------- wire

/// Encode ops into the patch root value `{ b: <base id>, o: [ops] }` (D30).
/// Op wire arrays: `[0, path, value]` (Set), `[1, path]` (Del),
/// `[2, path, at, remove, insert]` (Splice); path segments are strings or
/// integers.
///
/// # Errors
///
/// `ODP-SHAPE` when a path index or splice bound exceeds the exact wire
/// integer range — unreachable for ops produced by [`diff`] on decodable
/// payloads (a 256 MiB payload holds at most 2^28 elements), real only for
/// hand-built values.
pub fn ops_to_value(base_id: &str, ops: &[Op]) -> crate::Result<Value> {
    let mut root = BTreeMap::new();
    root.insert("b".to_string(), Value::String(base_id.to_string()));
    let mut o = Vec::with_capacity(ops.len());
    for op in ops {
        o.push(op_to_wire(op)?);
    }
    root.insert("o".to_string(), Value::Array(o));
    Ok(Value::Object(root))
}

/// Inverse of [`ops_to_value`]: parse the decoded patch root back into the
/// base id and the ops.
///
/// # Errors
///
/// - `ODP-SHAPE`: the root/ops/path shape does not match the wire contract
///   (wrong types, fractional indices, out-of-range integers).
/// - `ODP-OP`: an unknown op code.
/// - `ODP-KEY`: a reserved key in a path.
pub fn value_to_ops(value: &Value) -> crate::Result<(String, Vec<Op>)> {
    let Value::Object(m) = value else {
        return Err(crate::err("ODP-SHAPE", "patch root is not an object"));
    };
    let Some(Value::String(b)) = m.get("b") else {
        return Err(crate::err(
            "ODP-SHAPE",
            "patch root is missing the string `b` (base id)",
        ));
    };
    let Some(Value::Array(o)) = m.get("o") else {
        return Err(crate::err(
            "ODP-SHAPE",
            "patch root is missing the array `o` (ops)",
        ));
    };
    let mut ops = Vec::with_capacity(o.len());
    for op in o {
        ops.push(op_from_wire(op)?);
    }
    Ok((b.clone(), ops))
}

/// A wire integer: exact f64 integers go up to 2^53, but a cast-free
/// conversion runs through `u32` — and that is no practical bound: patch
/// indices come from decoded payloads, whose array lengths are bounded by
/// `MAX_BYTES` (256 MiB ⇒ ≤ 2^28 elements). Anything beyond fails encode.
fn usize_to_wire(i: usize) -> crate::Result<Value> {
    let u = u32::try_from(i).map_err(|_| {
        crate::err(
            "ODP-SHAPE",
            format!("index {i} exceeds the wire integer range"),
        )
    })?;
    Ok(Value::Number(f64::from(u)))
}

fn usize_from_wire(v: &Value, what: &str) -> crate::Result<usize> {
    let Value::Number(n) = v else {
        return Err(crate::err("ODP-SHAPE", format!("{what} must be a number")));
    };
    if !n.is_finite() || n.fract() != 0.0 || *n < 0.0 || *n > MAX_WIRE_INT {
        return Err(crate::err(
            "ODP-SHAPE",
            format!("{what} must be an exact non-negative integer (≤ 2^53)"),
        ));
    }
    // Cast-free conversion: the value is validated to be an exact
    // non-negative integer within 2^53, so its string rendering is the
    // canonical decimal form (no exponent, no fraction) and `u64` parses it
    // losslessly — no f64→int cast (the workspace is cast-free by policy).
    let u: u64 = n
        .to_string()
        .parse()
        .map_err(|_| crate::err("ODP-SHAPE", format!("{what} is out of the u64 range")))?;
    usize::try_from(u)
        .map_err(|_| crate::err("ODP-SHAPE", format!("{what} exceeds the platform usize")))
}

fn path_to_wire(path: &[Seg]) -> crate::Result<Value> {
    let mut segs = Vec::with_capacity(path.len());
    for seg in path {
        segs.push(match seg {
            Seg::Key(k) => Value::String(k.clone()),
            Seg::Idx(i) => usize_to_wire(*i)?,
        });
    }
    Ok(Value::Array(segs))
}

fn path_from_wire(v: &Value) -> crate::Result<Vec<Seg>> {
    let Value::Array(parts) = v else {
        return Err(crate::err("ODP-SHAPE", "an op path is an array"));
    };
    let mut segs = Vec::with_capacity(parts.len());
    for p in parts {
        segs.push(match p {
            Value::String(k) => {
                if !crate::wire::valid_object_key(k) {
                    return Err(crate::err(
                        "ODP-KEY",
                        format!("reserved key {k:?} cannot appear in a patch path"),
                    ));
                }
                Seg::Key(k.clone())
            }
            Value::Number(_) => Seg::Idx(usize_from_wire(p, "path index")?),
            _ => {
                return Err(crate::err(
                    "ODP-SHAPE",
                    "a path segment is a string or an integer",
                ));
            }
        });
    }
    Ok(segs)
}

fn op_to_wire(op: &Op) -> crate::Result<Value> {
    match op {
        Op::Set { path, value } => Ok(Value::Array(vec![
            Value::Number(0.0),
            path_to_wire(path)?,
            value.clone(),
        ])),
        Op::Del { path } => Ok(Value::Array(vec![Value::Number(1.0), path_to_wire(path)?])),
        Op::Splice {
            path,
            at,
            remove,
            insert,
        } => Ok(Value::Array(vec![
            Value::Number(2.0),
            path_to_wire(path)?,
            usize_to_wire(*at)?,
            usize_to_wire(*remove)?,
            Value::Array(insert.clone()),
        ])),
    }
}

fn op_from_wire(v: &Value) -> crate::Result<Op> {
    let Value::Array(parts) = v else {
        return Err(crate::err("ODP-SHAPE", "each op is an array"));
    };
    let Some((head, rest)) = parts.split_first() else {
        return Err(crate::err("ODP-SHAPE", "an op array cannot be empty"));
    };
    match usize_from_wire(head, "op code")? {
        0 => {
            let [path, value] = rest else {
                return Err(crate::err("ODP-SHAPE", "Set is [0, path, value]"));
            };
            Ok(Op::Set {
                path: path_from_wire(path)?,
                value: (*value).clone(),
            })
        }
        1 => {
            let [path] = rest else {
                return Err(crate::err("ODP-SHAPE", "Del is [1, path]"));
            };
            Ok(Op::Del {
                path: path_from_wire(path)?,
            })
        }
        2 => {
            let [path, at, remove, insert] = rest else {
                return Err(crate::err(
                    "ODP-SHAPE",
                    "Splice is [2, path, at, remove, insert]",
                ));
            };
            let Value::Array(items) = insert else {
                return Err(crate::err("ODP-SHAPE", "Splice insert must be an array"));
            };
            Ok(Op::Splice {
                path: path_from_wire(path)?,
                at: usize_from_wire(at, "splice at")?,
                remove: usize_from_wire(remove, "splice remove")?,
                insert: items.clone(),
            })
        }
        other => Err(crate::err("ODP-OP", format!("unknown op code {other}"))),
    }
}

// --------------------------------------------------------------- envelope

/// Encode a patch: the `ODP1` outer prefix followed by the ordinary ODF1
/// encoding of [`ops_to_value`] — "keep the full ODF1 payload so the existing
/// decoder works unchanged, with ODP1 as an outer 4-byte prefix" (D30).
///
/// # Errors
///
/// Passes through the encoder's own errors (`ODF-*`): a hand-built op value
/// that is not encodable (e.g. an invalid `Value::Ref` index) surfaces here.
pub fn encode_patch(base_id: &str, ops: &[Op]) -> crate::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(4 + 32);
    out.extend_from_slice(&MAGIC_PATCH);
    let root = ops_to_value(base_id, ops)?;
    out.extend_from_slice(&crate::encode(&root)?);
    Ok(out)
}

/// Decode a patch payload into its base id and ops. The inner ODF1 body is
/// decoded by the standard decoder (limits and hardening included), its
/// container dedup refs are resolved, and the ops shape is then parsed with
/// reserved-key paths rejected.
///
/// # Errors
///
/// - `ODP-MAGIC`: the payload does not start with `ODP1` (or has no body).
/// - `ODF-*`: the inner ODF1 body is malformed (pass-through).
/// - `ODP-SHAPE` / `ODP-OP` / `ODP-KEY`: the ops root is not a valid
///   `{ b, o }` payload (see [`value_to_ops`]).
pub fn decode_patch(bytes: &[u8]) -> crate::Result<(String, Vec<Op>)> {
    if !is_patch(bytes) {
        return Err(crate::err(
            "ODP-MAGIC",
            "patch payload does not start with the b\"ODP1\" magic",
        ));
    }
    // Refs are REAL here: the encoder dedups bit-exact-equal containers, so
    // e.g. an empty `insert` array aliases an empty `path` array as `Ref(i)`.
    // Decode with the shared table and resolve before parsing the ops shape.
    let (inner, shared) = crate::decode_with_shared(&bytes[4..])?;
    let inner = crate::refs::resolve_refs_with(&inner, &shared)?;
    value_to_ops(&inner)
}

/// Sniff the outer magic: true when `bytes` starts with `ODP1`.
#[must_use]
pub fn is_patch(bytes: &[u8]) -> bool {
    bytes.get(..4).is_some_and(|head| head == MAGIC_PATCH)
}

// ---------------------------------------------------------------- base id

/// The patch base id: the first 16 hex chars of the SHA-256 digest of
/// `bytes` (D30 — the client hashes the payload bytes it last decoded with
/// the same truncated SHA-256; not blake3, so the client has a single
/// algorithm). Lowercase hex.
#[must_use]
pub fn base_id(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    use std::fmt::Write as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        let _ = write!(out, "{b:02x}");
    }
    out
}
