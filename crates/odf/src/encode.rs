//! ODF encoder (spec §9.5, D11): version-prefixed, type-tagged, little-endian.
//!
//! Shared/cyclic references are handled without pointers: every container
//! occurrence is compared (bit-exact, [`crate::bitexact_eq`]) against recently
//! registered table slots; the first occurrence is written inline and occupies
//! the next slot, later occurrences become `Ref(slot)`. Slots are numbered in
//! pre-order, which the decoder reproduces exactly, so `Ref(i)` always points
//! at the i-th inline container.
//!
//! In-progress (ancestor) slots are skipped during the dedup scan: aliasing an
//! ancestor would collapse distinct structure and lose information on resolve.

use crate::wire::{tags, write_varu};
use crate::{err, Result, Value, MAX_BYTES, MAX_DEPTH, MAX_REFS};

/// How many recently registered slots the dedup scan inspects. A fixed window
/// keeps encoding linear and deterministic; duplicates further apart are
/// simply written inline (correct, just not shared).
const DEDUP_WINDOW: usize = 64;

pub(crate) fn encode_with_preamble(root: &Value, shared: &[(u32, Value)]) -> Result<Vec<u8>> {
    let n = shared.len();
    if n > MAX_REFS as usize {
        return Err(err(
            "ODF-REF",
            format!("shared table has {n} entries, limit is {MAX_REFS}"),
        ));
    }
    // dense indices 0..n, no duplicates, placed at their declared slot
    let mut table: Vec<Value> = vec![Value::Null; n];
    let mut seen = vec![false; n];
    for (i, v) in shared {
        let i = *i as usize;
        if i >= n || seen[i] {
            return Err(err(
                "ODF-REF",
                format!("shared table index {i} is out of range 0..{n} or duplicated"),
            ));
        }
        seen[i] = true;
        table[i] = v.clone();
    }
    // owned snapshot used to write the preamble (the encoder takes `table`)
    let entries = table.clone();

    let mut enc = Encoder {
        out: Vec::with_capacity(64),
        table,
        active: vec![false; n],
    };
    enc.out.extend_from_slice(&crate::MAGIC);
    write_varu(&mut enc.out, n as u32);

    // preamble: pre-resolved shared entries, written verbatim (their internal
    // containers occupy no slots; refs inside entries must target 0..n).
    for entry in &entries {
        enc.write_value(entry, 1, false)?;
    }
    // root: participates in slot numbering and dedup
    enc.write_value(root, 1, true)?;

    if enc.out.len() > MAX_BYTES {
        return Err(err(
            "ODF-SIZE",
            format!(
                "encoded payload is {} bytes, limit is {MAX_BYTES}",
                enc.out.len()
            ),
        ));
    }
    Ok(enc.out)
}

struct Encoder {
    out: Vec<u8>,
    /// Shared-slot table: slot → value. Grows as inline containers register.
    table: Vec<Value>,
    /// Parallel to `table`: true while a container's subtree is being written
    /// (i.e. the slot is an in-progress ancestor and must not be dedup-matched).
    active: Vec<bool>,
}

impl Encoder {
    fn write_value(&mut self, v: &Value, depth: u32, register: bool) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(err(
                "ODF-DEPTH",
                format!("value is nested deeper than the {MAX_DEPTH}-level limit"),
            ));
        }
        match v {
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                if register {
                    return self.write_shared_container(v, depth);
                }
                self.write_container_header(v, depth, register)
            }
            other => self.write_leaf(other),
        }
    }

    /// Dedup scan + slot registration, then an inline write.
    fn write_shared_container(&mut self, v: &Value, depth: u32) -> Result<()> {
        let start = self.table.len().saturating_sub(DEDUP_WINDOW);
        for slot in (start..self.table.len()).rev() {
            if self.active[slot] {
                continue; // never alias an in-progress ancestor
            }
            if crate::bitexact_eq(&self.table[slot], v) {
                return self.write_ref(slot as u32);
            }
        }
        if self.table.len() >= MAX_REFS as usize {
            return Err(err(
                "ODF-REF",
                format!("more than {MAX_REFS} shared slots would be registered"),
            ));
        }
        let slot = self.table.len();
        self.table.push(v.clone());
        self.active.push(true);
        let r = self.write_container_header(v, depth, true);
        // the slot stays registered; only the "in progress" flag is cleared.
        // Index by `slot`, not `last_mut()`: nested containers registered while
        // writing the subtree must keep their own flags untouched.
        self.active[slot] = false;
        r
    }

    /// Write the tag + count + children of a container value.
    fn write_container_header(&mut self, v: &Value, depth: u32, register: bool) -> Result<()> {
        match v {
            Value::Array(items) => {
                self.out.push(tags::ARRAY);
                write_varu(&mut self.out, items.len() as u32);
                for item in items {
                    self.write_value(item, depth + 1, register)?;
                }
            }
            Value::Set(items) => {
                self.out.push(tags::SET);
                write_varu(&mut self.out, items.len() as u32);
                for item in items {
                    self.write_value(item, depth + 1, register)?;
                }
            }
            Value::Object(map) => {
                self.out.push(tags::OBJECT);
                write_varu(&mut self.out, map.len() as u32);
                for (k, val) in map {
                    if !crate::wire::valid_object_key(k) {
                        return Err(err(
                            "ODF-KEY",
                            format!(
                                "reserved object key `{k}` (§17.1: use a Map for data-level keys)"
                            ),
                        ));
                    }
                    write_varu(&mut self.out, k.len() as u32);
                    self.out.extend_from_slice(k.as_bytes());
                    self.write_value(val, depth + 1, register)?;
                }
            }
            Value::Map(pairs) => {
                self.out.push(tags::MAP);
                write_varu(&mut self.out, pairs.len() as u32);
                for (k, val) in pairs {
                    self.write_value(k, depth + 1, register)?;
                    self.write_value(val, depth + 1, register)?;
                }
            }
            other => unreachable!("write_container_header called on {other:?}"),
        }
        Ok(())
    }

    fn write_ref(&mut self, slot: u32) -> Result<()> {
        self.out.push(tags::REF);
        write_varu(&mut self.out, slot);
        Ok(())
    }

    /// Non-container payload (primitives, leaves, refs, deferred).
    fn write_leaf(&mut self, v: &Value) -> Result<()> {
        match v {
            Value::Null => self.out.push(tags::NULL),
            Value::Undefined => self.out.push(tags::UNDEFINED),
            Value::Bool(b) => {
                self.out.push(tags::BOOL);
                self.out.push(*b as u8);
            }
            Value::Number(x) => {
                self.out.push(tags::NUMBER);
                self.out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            Value::BigInt(s) => {
                if !crate::wire::validate_bigint(s) {
                    return Err(err(
                        "ODF-TAG",
                        format!("invalid BigInt decimal string `{s}` (expected `-?(0|[0-9]+)`)"),
                    ));
                }
                self.out.push(tags::BIGINT);
                write_varu(&mut self.out, s.len() as u32);
                self.out.extend_from_slice(s.as_bytes());
            }
            Value::String(s) => {
                self.out.push(tags::STRING);
                write_varu(&mut self.out, s.len() as u32);
                self.out.extend_from_slice(s.as_bytes());
            }
            Value::Date(ms) => {
                self.out.push(tags::DATE);
                self.out.extend_from_slice(&ms.to_le_bytes());
            }
            Value::Url(s) => {
                self.out.push(tags::URL);
                write_varu(&mut self.out, s.len() as u32);
                self.out.extend_from_slice(s.as_bytes());
            }
            Value::RegExp(source, flags) => {
                self.out.push(tags::REGEXP);
                write_varu(&mut self.out, source.len() as u32);
                self.out.extend_from_slice(source.as_bytes());
                write_varu(&mut self.out, flags.len() as u32);
                self.out.extend_from_slice(flags.as_bytes());
            }
            Value::TypedArray(kind, data) => {
                let size = kind.element_size();
                if data.len() % size != 0 {
                    return Err(err(
                        "ODF-TAG",
                        format!(
                            "typed array payload length {} is not a multiple of the {}-byte element size",
                            data.len(),
                            size
                        ),
                    ));
                }
                self.out.push(tags::TYPED_ARRAY);
                self.out.push(*kind as u8);
                write_varu(&mut self.out, data.len() as u32);
                self.out.extend_from_slice(data);
            }
            Value::Error {
                name,
                message,
                stack,
            } => {
                self.out.push(tags::ERROR);
                write_varu(&mut self.out, name.len() as u32);
                self.out.extend_from_slice(name.as_bytes());
                write_varu(&mut self.out, message.len() as u32);
                self.out.extend_from_slice(message.as_bytes());
                match stack {
                    Some(s) => {
                        self.out.push(1);
                        write_varu(&mut self.out, s.len() as u32);
                        self.out.extend_from_slice(s.as_bytes());
                    }
                    None => self.out.push(0),
                }
            }
            Value::Ref(i) => {
                if *i == u32::MAX {
                    return Err(err(
                        "ODF-REF",
                        "Ref(u32::MAX) is the cycle-cut marker and is not encodable",
                    ));
                }
                if (*i as usize) >= self.table.len() {
                    return Err(err(
                        "ODF-REF",
                        format!(
                            "Ref({i}) points outside the shared table ({} slot(s) registered)",
                            self.table.len()
                        ),
                    ));
                }
                self.write_ref(*i)?;
            }
            Value::Deferred { id } => {
                self.out.push(tags::DEFERRED);
                write_varu(&mut self.out, *id);
            }
            // containers never reach write_leaf
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                unreachable!("write_leaf called on a container")
            }
        }
        Ok(())
    }
}
