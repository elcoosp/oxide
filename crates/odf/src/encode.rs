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
use crate::{MAX_BYTES, MAX_DEPTH, MAX_REFS, Result, Value, err};

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
        enc.write_tree(entry, 1, false)?;
    }
    // root: participates in slot numbering and dedup
    enc.write_tree(root, 1, true)?;

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
    /// Write one value tree iteratively over an explicit frame stack (§17.1):
    /// a hostile `MAX_DEPTH`-deep value must fail with `ODF-DEPTH`, not by
    /// overflowing the host thread's stack before the guard can fire.
    ///
    /// `register` selects slot numbering + dedup (root subtrees) versus a
    /// verbatim write (preamble entries, whose containers occupy no slots).
    /// The flag applies to the whole subtree, exactly as the former
    /// recursion threaded it through every level.
    fn write_tree(&mut self, root: &Value, depth: u32, register: bool) -> Result<()> {
        // Seed: leaves write flat; containers emit their header and push.
        if !self.visit(root, depth, register)? {
            return Ok(());
        }
        let mut stack: Vec<EFrame<'_>> = Vec::new();
        self.push_frame(&mut stack, root, depth, register)?;
        while stack.last().is_some() {
            let next = stack.last_mut().expect("just checked").advance();
            let Some((child, key)) = next else {
                let done = stack.pop().expect("just checked");
                if let Some(slot) = done.slot_to_close {
                    // The slot stays registered; only the "in progress"
                    // flag is cleared. Index by `slot`, not `last_mut()`:
                    // nested containers registered while writing the
                    // subtree keep their own flags untouched.
                    self.active[slot] = false;
                }
                continue;
            };
            if let Some(k) = key {
                if !crate::wire::valid_object_key(k) {
                    return Err(err(
                        "ODF-KEY",
                        format!("reserved object key `{k}` (§17.1: use a Map for data-level keys)"),
                    ));
                }
                write_varu(&mut self.out, k.len() as u32);
                self.out.extend_from_slice(k.as_bytes());
            }
            let child_depth = stack.last().map(|f| f.depth + 1).unwrap_or(1);
            if self.visit(child, child_depth, register)? {
                self.push_frame(&mut stack, child, child_depth, register)?;
            }
        }
        Ok(())
    }

    /// Emit one value's tag (and, for containers, its header): leaves and
    /// dedup-matched containers finish immediately (`Ok(false)`); a new
    /// inline container needs a frame (`Ok(true)`).
    fn visit(&mut self, v: &Value, depth: u32, register: bool) -> Result<bool> {
        if depth > MAX_DEPTH {
            return Err(err(
                "ODF-DEPTH",
                format!("value is nested deeper than the {MAX_DEPTH}-level limit"),
            ));
        }
        match v {
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                if register {
                    if let Some(wrote_ref) = self.dedup_or_register(v)? {
                        debug_assert!(wrote_ref);
                        return Ok(false);
                    }
                }
                self.write_header(v)?;
                Ok(true)
            }
            other => {
                self.write_leaf(other)?;
                Ok(false)
            }
        }
    }

    /// Dedup scan + slot registration for one container. Returns `Ok(Some)`
    /// when a `Ref(slot)` was written (nothing more to do), `Ok(None)` when
    /// the container was registered inline and still needs its header.
    fn dedup_or_register(&mut self, v: &Value) -> Result<Option<bool>> {
        let start = self.table.len().saturating_sub(DEDUP_WINDOW);
        for slot in (start..self.table.len()).rev() {
            if self.active[slot] {
                continue; // never alias an in-progress ancestor
            }
            if crate::bitexact_eq(&self.table[slot], v) {
                self.write_ref(slot as u32)?;
                return Ok(Some(true));
            }
        }
        if self.table.len() >= MAX_REFS as usize {
            return Err(err(
                "ODF-REF",
                format!("more than {MAX_REFS} shared slots would be registered"),
            ));
        }
        // Register inline: the slot index is `table.len()`, recovered by
        // `push_frame` when the frame is created (nothing mutates the
        // table between registration and the push: only header bytes are
        // emitted in between).
        self.table.push(v.clone());
        self.active.push(true);
        Ok(None)
    }

    /// Push the frame for a freshly headered container.
    fn push_frame<'a>(
        &self,
        stack: &mut Vec<EFrame<'a>>,
        v: &'a Value,
        depth: u32,
        register: bool,
    ) -> Result<()> {
        // `register` trees number this container as the latest slot (the
        // dedup step just pushed it); preamble trees occupy no slots.
        let slot_to_close = if register {
            Some(self.table.len() - 1)
        } else {
            None
        };
        stack.push(EFrame::for_value(v, depth, slot_to_close));
        Ok(())
    }

    /// Write a container's tag + entry count. Children follow from the
    /// frame driver; keys are emitted per child by `write_tree`.
    fn write_header(&mut self, v: &Value) -> Result<()> {
        match v {
            Value::Array(items) => {
                self.out.push(tags::ARRAY);
                write_varu(&mut self.out, items.len() as u32);
            }
            Value::Set(items) => {
                self.out.push(tags::SET);
                write_varu(&mut self.out, items.len() as u32);
            }
            Value::Object(map) => {
                self.out.push(tags::OBJECT);
                write_varu(&mut self.out, map.len() as u32);
            }
            Value::Map(pairs) => {
                self.out.push(tags::MAP);
                write_varu(&mut self.out, pairs.len() as u32);
            }
            other => unreachable!("write_header called on {other:?}"),
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

/// One container under construction on the explicit work stack.
/// Map pairs count as two children (key, then value); object keys are bare
/// strings emitted alongside their value child.
struct EFrame<'a> {
    depth: u32,
    /// Active-table slot to deactivate when this frame completes (`None`
    /// for preamble subtrees, which occupy no slots).
    slot_to_close: Option<usize>,
    kind: EKind<'a>,
}

enum EKind<'a> {
    Array {
        items: &'a [Value],
        idx: usize,
    },
    Set {
        items: &'a [Value],
        idx: usize,
    },
    Object {
        entries: Vec<(&'a str, &'a Value)>,
        idx: usize,
    },
    Map {
        pairs: &'a [(Value, Value)],
        idx: usize,
    },
}

impl<'a> EFrame<'a> {
    fn for_value(v: &'a Value, depth: u32, slot_to_close: Option<usize>) -> Self {
        let kind = match v {
            Value::Array(items) => EKind::Array { items, idx: 0 },
            Value::Set(items) => EKind::Set { items, idx: 0 },
            Value::Object(map) => EKind::Object {
                entries: map.iter().map(|(k, val)| (k.as_str(), val)).collect(),
                idx: 0,
            },
            Value::Map(pairs) => EKind::Map { pairs, idx: 0 },
            other => unreachable!("frame built for {other:?}"),
        };
        EFrame {
            depth,
            slot_to_close,
            kind,
        }
    }

    /// Next child to visit plus, for objects, the key bytes to emit first.
    /// Returns `None` when the frame is exhausted.
    fn advance(&mut self) -> Option<(&'a Value, Option<&'a str>)> {
        match &mut self.kind {
            EKind::Array { items, idx } | EKind::Set { items, idx } => {
                let i = *idx;
                if i >= items.len() {
                    return None;
                }
                *idx = i + 1;
                Some((&items[i], None))
            }
            EKind::Object { entries, idx } => {
                let i = *idx;
                if i >= entries.len() {
                    return None;
                }
                *idx = i + 1;
                let (k, v) = entries[i];
                Some((v, Some(k)))
            }
            EKind::Map { pairs, idx } => {
                let i = *idx;
                if i >= pairs.len() * 2 {
                    return None;
                }
                *idx = i + 1;
                let (k, v) = &pairs[i / 2];
                Some((if i % 2 == 0 { k } else { v }, None))
            }
        }
    }
}
