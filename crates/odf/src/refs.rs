//! Reference resolution: turn `Value::Ref(i)` markers back into the values
//! they point at (spec §9.5 shared/cyclic references, §7.1 deferred slots).
//!
//! Slot model (see `docs/format.md`):
//! - slots `0..shared_len` are the payload's pre-resolved shared entries,
//! - slots `shared_len..` are the root's inline containers in pre-order.
//!
//! Because `Value` is a plain tree, true cycles are represented by leaving a
//! `Value::Ref(i)` marker in place at the back-edge ("cycle-cut marker").
//! Resolution is therefore cycle-tolerant: cyclic payloads resolve without
//! error, and only pathological expansion beyond [`crate::MAX_DEPTH`]
//! (acyclic DAGs unrolled past the limit) fails with `ODF-CYCLE`.

use crate::{err, Result, Value, MAX_DEPTH};
use std::collections::BTreeSet;

/// Resolve `Value::Ref(i)` markers against the value's own virtual shared
/// table (the i-th inline container in pre-order). Use
/// [`resolve_refs_with`] for payloads decoded via [`crate::decode_with_shared`].
pub fn resolve_refs(value: &Value) -> Result<Value> {
    resolve_refs_with(value, &[])
}

/// Resolve `Value::Ref(i)` markers where slots `0..shared.len()` refer to a
/// pre-resolved shared table (as produced by [`crate::encode_with_shared`])
/// and later slots refer to inline containers of `value` in pre-order.
pub fn resolve_refs_with(value: &Value, shared: &[Value]) -> Result<Value> {
    let mut rs = Resolver {
        shared,
        memo: vec![None; shared.len()],
        active: BTreeSet::new(),
        counter: shared.len(),
    };
    rs.walk_tree(value, 1)
}

struct Resolver<'a> {
    /// Pre-resolved shared entries (slots `0..shared.len()`).
    shared: &'a [Value],
    /// Resolved subtree per slot, filled post-order. Slots for the root's own
    /// inline containers are appended as the walk assigns them.
    memo: Vec<Option<Value>>,
    /// Slots currently being expanded along the active path.
    active: BTreeSet<usize>,
    /// Next slot index for an inline container (`shared.len()`-based).
    counter: usize,
}

impl<'a> Resolver<'a> {
    /// Walk the root tree: inline containers take the next slot in pre-order.
    ///
    /// The depth guard applies to containers only: leaves never recurse here,
    /// so they cannot contribute to expansion depth (mirrors the encoder, where
    /// only nesting structure counts toward `MAX_DEPTH`).
    fn walk_tree(&mut self, v: &Value, depth: u32) -> Result<Value> {
        match v {
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                if depth > MAX_DEPTH {
                    return Err(err(
                        "ODF-CYCLE",
                        format!("reference expansion exceeds the {MAX_DEPTH}-level limit"),
                    ));
                }
                let slot = self.counter;
                self.counter += 1;
                self.memo.push(None);
                self.active.insert(slot);
                let resolved = self.build(v, depth)?;
                self.active.remove(&slot);
                self.memo[slot] = Some(resolved.clone());
                Ok(resolved)
            }
            Value::Ref(i) => self.resolve_ref(*i, depth),
            other => Ok(other.clone()),
        }
    }

    /// Build a resolved container from an inline one (children via `walk_tree`).
    fn build(&mut self, v: &Value, depth: u32) -> Result<Value> {
        Ok(match v {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.walk_tree(it, depth + 1)?);
                }
                Value::Array(out)
            }
            Value::Set(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.walk_tree(it, depth + 1)?);
                }
                Value::Set(out)
            }
            Value::Object(map) => {
                let mut out = std::collections::BTreeMap::new();
                for (k, val) in map {
                    out.insert(k.clone(), self.walk_tree(val, depth + 1)?);
                }
                Value::Object(out)
            }
            Value::Map(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (k, val) in pairs {
                    out.push((
                        self.walk_tree(k, depth + 1)?,
                        self.walk_tree(val, depth + 1)?,
                    ));
                }
                Value::Map(out)
            }
            other => unreachable!("build called on {other:?}"),
        })
    }

    /// Walk a shared entry: its internal containers occupy no slots, so refs
    /// inside may only target other shared entries (or inline-registered slots
    /// that are already complete).
    fn walk_shared(&mut self, v: &Value, depth: u32) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(err(
                "ODF-CYCLE",
                format!("reference expansion exceeds the {MAX_DEPTH}-level limit"),
            ));
        }
        match v {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.walk_shared(it, depth + 1)?);
                }
                Ok(Value::Array(out))
            }
            Value::Set(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.walk_shared(it, depth + 1)?);
                }
                Ok(Value::Set(out))
            }
            Value::Object(map) => {
                let mut out = std::collections::BTreeMap::new();
                for (k, val) in map {
                    out.insert(k.clone(), self.walk_shared(val, depth + 1)?);
                }
                Ok(Value::Object(out))
            }
            Value::Map(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (k, val) in pairs {
                    out.push((
                        self.walk_shared(k, depth + 1)?,
                        self.walk_shared(val, depth + 1)?,
                    ));
                }
                Ok(Value::Map(out))
            }
            Value::Ref(i) => self.resolve_ref(*i, depth),
            other => Ok(other.clone()),
        }
    }

    fn resolve_ref(&mut self, i: u32, depth: u32) -> Result<Value> {
        // the documented cycle-cut marker is passed through untouched
        if i == u32::MAX {
            return Ok(Value::Ref(i));
        }
        let slot = i as usize;
        if slot < self.shared.len() {
            // shared entry: expand once, memoize, cut cycles among entries
            if let Some(m) = &self.memo[slot] {
                return Ok(m.clone());
            }
            if !self.active.insert(slot) {
                return Ok(Value::Ref(i));
            }
            let entry = self.shared[slot].clone();
            let resolved = self.walk_shared(&entry, depth + 1)?;
            self.active.remove(&slot);
            self.memo[slot] = Some(resolved.clone());
            Ok(resolved)
        } else if slot < self.counter {
            // inline container slot: either an in-progress ancestor (back-edge
            // → leave the marker) or a completed, memoized subtree
            if self.active.contains(&slot) {
                return Ok(Value::Ref(i));
            }
            match &self.memo[slot] {
                Some(m) => Ok(m.clone()),
                None => Err(err("ODF-REF", format!("Ref({i}) cannot be resolved"))),
            }
        } else {
            Err(err(
                "ODF-REF",
                format!(
                    "Ref({i}) points outside the shared table ({} slot(s))",
                    self.counter
                ),
            ))
        }
    }
}
