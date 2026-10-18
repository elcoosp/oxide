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
//!
//! The walk is iterative over an explicit work stack (§17.1 hardening): a
//! hostile payload nested `MAX_DEPTH`-deep must fail with `ODF-CYCLE`, not
//! by overflowing the host thread's stack before the guard can fire.

use crate::{MAX_DEPTH, Result, Value, err};
use std::collections::{BTreeMap, BTreeSet};

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
    Resolver {
        shared,
        memo: vec![None; shared.len()],
        active: BTreeSet::new(),
        counter: shared.len(),
    }
    .run(value)
}

/// One unevaluated child of a container frame. Map pairs flatten to
/// alternating key/value children so a single output vec rebuilds them.
#[derive(Debug)]
enum Child<'a> {
    Val(&'a Value),
    Key(&'a Value),
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Shape {
    Array,
    Set,
    Object,
    Map,
}

/// How a frame's result is filed on completion.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// Inline container: pre-order slot; memoized and cycle-tracked.
    Inline(usize),
    /// Shared-table entry: memoized under the table index, cycle-tracked.
    SharedEntry(usize),
    /// Interior container of a shared entry: occupies no slot, rebuilt on
    /// every expansion (mirrors the former recursive `walk_shared`).
    Anon,
}

/// An in-progress container expansion on the explicit work stack.
struct Frame<'a> {
    slot: Slot,
    depth: u32,
    /// Shared subtrees check the depth guard on *every* node (leaves
    /// included); inline subtrees check containers only (mirrors the former
    /// recursive `walk_shared` / `walk_tree` pair).
    strict: bool,
    shape: Shape,
    /// Object keys in order (cloned eagerly; object keys resolve to
    /// themselves and never recurse).
    obj_keys: Vec<String>,
    children: Vec<Child<'a>>,
    next: usize,
    out: Vec<Value>,
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
    fn run(&mut self, root: &'a Value) -> Result<Value> {
        let mut stack: Vec<Frame<'a>> = Vec::new();
        match root {
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                self.push_inline(&mut stack, root, 1)?;
            }
            Value::Ref(i) => {
                if let Some(v) = self.resolve_ref(&mut stack, *i, 1)? {
                    debug_assert!(stack.is_empty());
                    return Ok(v);
                }
            }
            other => return Ok(other.clone()),
        }
        loop {
            let top = stack.last_mut().expect("work stack non-empty");
            if top.next < top.children.len() {
                let idx = top.next;
                top.next += 1;
                // Reborrow the child without holding `top` across the push
                // below (pushing may reallocate the stack vec).
                let (strict, depth, is_key) = {
                    let t = stack.last().expect("work stack non-empty");
                    (t.strict, t.depth, matches!(t.children[idx], Child::Key(_)))
                };
                let node: &'a Value = match &stack.last().expect("non-empty").children[idx] {
                    Child::Val(v) | Child::Key(v) => v,
                };
                let _ = is_key;
                self.step_node(&mut stack, node, strict, depth)?;
            } else {
                let frame = stack.pop().expect("work stack non-empty");
                let built = Self::build_shape(frame.shape, frame.obj_keys, frame.out);
                match frame.slot {
                    Slot::Inline(slot) | Slot::SharedEntry(slot) => {
                        self.active.remove(&slot);
                        self.memo[slot] = Some(built.clone());
                    }
                    Slot::Anon => {}
                }
                if stack.is_empty() {
                    return Ok(built);
                }
                stack.last_mut().expect("non-empty").out.push(built);
            }
        }
    }

    /// Process one child node of the top frame: leaves and immediately
    /// resolvable refs append to the top frame's output, containers and
    /// shared-entry refs push new frames.
    fn step_node(
        &mut self,
        stack: &mut Vec<Frame<'a>>,
        node: &'a Value,
        strict: bool,
        depth: u32,
    ) -> Result<()> {
        let child_depth = depth + 1;
        match node {
            Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                if child_depth > MAX_DEPTH {
                    return Err(Self::too_deep());
                }
                if strict {
                    self.push_anon(stack, node, child_depth)?;
                } else {
                    self.push_inline(stack, node, child_depth)?;
                }
                Ok(())
            }
            Value::Ref(i) => {
                if strict && child_depth > MAX_DEPTH {
                    return Err(Self::too_deep());
                }
                if let Some(v) = self.resolve_ref(stack, *i, depth)? {
                    stack.last_mut().expect("non-empty").out.push(v);
                }
                Ok(())
            }
            other => {
                if strict && child_depth > MAX_DEPTH {
                    return Err(Self::too_deep());
                }
                stack.last_mut().expect("non-empty").out.push(other.clone());
                Ok(())
            }
        }
    }

    /// Push an inline container frame (takes the next pre-order slot).
    ///
    /// The depth guard applies to containers only: leaves never recurse, so
    /// they cannot contribute to expansion depth (mirrors the encoder, where
    /// only nesting structure counts toward `MAX_DEPTH`).
    fn push_inline(&mut self, stack: &mut Vec<Frame<'a>>, v: &'a Value, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Self::too_deep());
        }
        let slot = self.counter;
        self.counter += 1;
        self.memo.push(None);
        self.active.insert(slot);
        stack.push(Frame::new(Slot::Inline(slot), false, v, depth));
        Ok(())
    }

    /// Push an interior container of a shared entry: no slot, strict depth
    /// accounting, result used once and discarded.
    fn push_anon(&mut self, stack: &mut Vec<Frame<'a>>, v: &'a Value, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Self::too_deep());
        }
        stack.push(Frame::new(Slot::Anon, true, v, depth));
        Ok(())
    }

    /// Resolve a `Ref(i)` seen at `depth` (the ref's own depth). Returns
    /// `Ok(None)` when a shared-entry frame was pushed — its completion
    /// delivers the value to the parent frame.
    fn resolve_ref(
        &mut self,
        stack: &mut Vec<Frame<'a>>,
        i: u32,
        depth: u32,
    ) -> Result<Option<Value>> {
        // The documented cycle-cut marker is passed through untouched.
        if i == u32::MAX {
            return Ok(Some(Value::Ref(i)));
        }
        let slot = i as usize;
        if slot < self.shared.len() {
            // Shared entry: expand once, memoize, cut cycles among entries.
            if let Some(m) = &self.memo[slot] {
                return Ok(Some(m.clone()));
            }
            if !self.active.insert(slot) {
                return Ok(Some(Value::Ref(i)));
            }
            let entry: &'a Value = &self.shared[slot];
            let frame_depth = depth + 1;
            if frame_depth > MAX_DEPTH {
                self.active.remove(&slot);
                return Err(Self::too_deep());
            }
            match entry {
                Value::Array(_) | Value::Object(_) | Value::Map(_) | Value::Set(_) => {
                    stack.push(Frame::new(
                        Slot::SharedEntry(slot),
                        true,
                        entry,
                        frame_depth,
                    ));
                    Ok(None)
                }
                // A leaf entry resolves flat (the former `walk_shared`
                // returned `other.clone()` for leaves after its depth
                // check, which just passed via `frame_depth`).
                other => {
                    self.active.remove(&slot);
                    let v = other.clone();
                    self.memo[slot] = Some(v.clone());
                    Ok(Some(v))
                }
            }
        } else if slot < self.counter {
            // Inline container slot: either an in-progress ancestor
            // (back-edge → leave the marker) or a completed subtree.
            if self.active.contains(&slot) {
                return Ok(Some(Value::Ref(i)));
            }
            match &self.memo[slot] {
                Some(m) => Ok(Some(m.clone())),
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

    fn too_deep() -> oxide_diagnostics::OxideError {
        err(
            "ODF-CYCLE",
            format!("reference expansion exceeds the {MAX_DEPTH}-level limit"),
        )
    }

    fn build_shape(shape: Shape, obj_keys: Vec<String>, out: Vec<Value>) -> Value {
        match shape {
            Shape::Array => Value::Array(out),
            Shape::Set => Value::Set(out),
            Shape::Object => {
                let mut map = BTreeMap::new();
                for (k, v) in obj_keys.into_iter().zip(out) {
                    map.insert(k, v);
                }
                Value::Object(map)
            }
            Shape::Map => {
                let mut pairs = Vec::with_capacity(out.len() / 2);
                let mut it = out.into_iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    pairs.push((k, v));
                }
                Value::Map(pairs)
            }
        }
    }
}

impl<'a> Frame<'a> {
    fn new(slot: Slot, strict: bool, v: &'a Value, depth: u32) -> Self {
        match v {
            Value::Array(items) => Frame {
                slot,
                depth,
                strict,
                shape: Shape::Array,
                obj_keys: Vec::new(),
                children: items.iter().map(Child::Val).collect(),
                next: 0,
                out: Vec::with_capacity(items.len()),
            },
            Value::Set(items) => Frame {
                slot,
                depth,
                strict,
                shape: Shape::Set,
                obj_keys: Vec::new(),
                children: items.iter().map(Child::Val).collect(),
                next: 0,
                out: Vec::with_capacity(items.len()),
            },
            Value::Object(map) => {
                let mut obj_keys = Vec::with_capacity(map.len());
                let mut children = Vec::with_capacity(map.len());
                for (k, val) in map {
                    obj_keys.push(k.clone());
                    children.push(Child::Val(val));
                }
                Frame {
                    slot,
                    depth,
                    strict,
                    shape: Shape::Object,
                    obj_keys,
                    children,
                    next: 0,
                    out: Vec::with_capacity(map.len()),
                }
            }
            Value::Map(pairs) => {
                let mut children = Vec::with_capacity(pairs.len() * 2);
                for (k, val) in pairs {
                    children.push(Child::Key(k));
                    children.push(Child::Val(val));
                }
                Frame {
                    slot,
                    depth,
                    strict,
                    shape: Shape::Map,
                    obj_keys: Vec::new(),
                    children,
                    next: 0,
                    out: Vec::with_capacity(pairs.len() * 2),
                }
            }
            other => unreachable!("frame built for {other:?}"),
        }
    }
}
