//! Tier A demand-driven graph (spec §5.1, §5.2, §5.6, D6).
//!
//! Design notes:
//! - **Memoization + epochs**: every memoized node records the graph `version`
//!   (epoch) it was computed in. `invalidate` bumps the epoch first, so any node
//!   that was computed while an invalidation raced it (old epoch) recomputes on
//!   the next `get` — the "never serve stale results" guarantee of §5.6. Nodes
//!   untouched by an invalidation are re-stamped so the bump does not flush them.
//! - **Single flight**: concurrent identical `get`s join one in-flight task
//!   (mutex + condvar); the first arrival computes, the rest block on the result.
//! - **Early cutoff (§5.1)**: `invalidate` eagerly revalidates affected nodes
//!   through the compute registry; a node whose recomputed output hash matches
//!   the old one keeps its dependents memoized.
//! - **Dependency recording**: reads go through per-computation recording
//!   wrappers; `Input::File`/`DirListing`/`Env`/`ConfigKey`/`ToolVersion` are
//!   indexed by identity for invalidation, `Input::Node` edges populate
//!   `reverse_deps`.

use crate::err;
use crate::{
    CancelToken, Compute, ComputeCtx, ConfigTracker, EnvTracker, FileTracker, Input, NodeKey,
    NodeOutput, OxideError, Result,
};
use dashmap::DashMap;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

/// Identity of a tracked input for invalidation matching: two `Input`s match
/// when they point at the same resource, regardless of the observed hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum InputId {
    File(PathBuf),
    Dir(PathBuf),
    Env(String),
    Config(String),
    Tool(String),
    Node(NodeKey),
}

impl InputId {
    pub(crate) fn of(input: &Input) -> Self {
        match input {
            Input::File { path, .. } => Self::File(path.clone()),
            Input::DirListing { path, .. } => Self::Dir(path.clone()),
            Input::Env { key, .. } => Self::Env(key.clone()),
            Input::ConfigKey { key, .. } => Self::Config(key.clone()),
            Input::ToolVersion { name, .. } => Self::Tool(name.clone()),
            Input::Node(k, _) => Self::Node(k.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// per-computation recording wrappers
// ---------------------------------------------------------------------------

struct RecFs<'a> {
    inner: &'a mut dyn FileTracker,
    deps: &'a RefCell<BTreeSet<Input>>,
}

impl FileTracker for RecFs<'_> {
    fn read_file(&mut self, path: &Path) -> Result<Vec<u8>> {
        let data = self.inner.read_file(path)?;
        self.deps.borrow_mut().insert(Input::File {
            path: path.to_path_buf(),
            hash: blake3::hash(&data),
        });
        Ok(data)
    }

    fn list_dir(&mut self, path: &Path) -> Result<Vec<PathBuf>> {
        let listing = self.inner.list_dir(path)?;
        let mut h = blake3::Hasher::new();
        for p in &listing {
            h.update(p.to_string_lossy().as_bytes());
            h.update(&[0]);
        }
        self.deps.borrow_mut().insert(Input::DirListing {
            path: path.to_path_buf(),
            listing_hash: h.finalize(),
        });
        Ok(listing)
    }
}

struct RecEnv<'a> {
    inner: &'a mut dyn EnvTracker,
    deps: &'a RefCell<BTreeSet<Input>>,
}

impl EnvTracker for RecEnv<'_> {
    fn get(&mut self, key: &str) -> Option<String> {
        let v = self.inner.get(key)?;
        self.deps.borrow_mut().insert(Input::Env {
            key: key.to_string(),
            value_hash: blake3::hash(v.as_bytes()),
        });
        Some(v)
    }
}

struct RecCfg<'a> {
    inner: &'a mut dyn ConfigTracker,
    deps: &'a RefCell<BTreeSet<Input>>,
}

impl ConfigTracker for RecCfg<'_> {
    fn get(&mut self, key: &str) -> Option<serde_json::Value> {
        let v = self.inner.get(key)?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        self.deps.borrow_mut().insert(Input::ConfigKey {
            key: key.to_string(),
            value_hash: blake3::hash(&bytes),
        });
        Some(v)
    }
}

// ---------------------------------------------------------------------------
// single-flight machinery
// ---------------------------------------------------------------------------

struct Task {
    state: Mutex<TaskState>,
    cv: Condvar,
}

enum TaskState {
    Running,
    Done(Arc<std::result::Result<Arc<NodeOutput>, OxideError>>),
}

fn clone_result(r: &Result<Arc<NodeOutput>>) -> Result<Arc<NodeOutput>> {
    match r {
        Ok(o) => Ok(o.clone()),
        Err(e) => Err(OxideError(e.0.clone())),
    }
}

thread_local! {
    /// Nodes currently computing on this thread — used to reject dependency
    /// cycles (`A` demanding `B` demanding `A`) with `GRAPH-CYCLE`.
    static COMPUTE_STACK: RefCell<Vec<NodeKey>> = const { RefCell::new(Vec::new()) };
}

type FsFactory = Arc<dyn Fn() -> Box<dyn FileTracker> + Send + Sync>;
type EnvFactory = Arc<dyn Fn() -> Box<dyn EnvTracker> + Send + Sync>;
type CfgFactory = Arc<dyn Fn() -> Box<dyn ConfigTracker> + Send + Sync>;

/// Walk status of a closure node during invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Not reachable through any changed path so far.
    Unchecked,
    /// Must be recomputed (seed, or a dependency changed output).
    RecheckNeeded,
    /// Output hash unchanged — dependents stay memoized (early cutoff, §5.1).
    Survivor,
    /// Recomputed with a *changed* output: dependents must revalidate.
    RecomputedNew,
    /// Memo dropped (compute failed or no registered compute).
    Invalidated,
}

/// The demand-driven graph (Tier A). `get` computes on demand, memoizes,
/// tracks deps, and applies **early cutoff**: if a recomputed node yields the
/// same output hash, dependents are not invalidated (§5.1).
#[derive(Default)]
pub struct DemandGraph {
    nodes: DashMap<NodeKey, NodeOutput>,
    reverse_deps: DashMap<NodeKey, BTreeSet<NodeKey>>,
    pub cancel: CancelToken,
    /// Epoch each memoized node was computed in (see module docs).
    epochs: DashMap<NodeKey, u64>,
    in_flight: DashMap<NodeKey, Arc<Task>>,
    input_index: DashMap<InputId, BTreeSet<NodeKey>>,
    registry: DashMap<NodeKey, Arc<dyn Compute>>,
    version: AtomicU64,
    fs_factory: RwLock<Option<FsFactory>>,
    env_factory: RwLock<Option<EnvFactory>>,
    config_factory: RwLock<Option<CfgFactory>>,
}

impl DemandGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current epoch (`version`). Bumped by [`DemandGraph::invalidate`]; nodes
    /// memoized in an older epoch recompute on next `get`.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Register (or replace) a compute for `key` so that [`ComputeCtx::dep`]
    /// can demand-compute it and `invalidate` can eagerly revalidate it.
    pub fn register(&self, key: NodeKey, compute: Arc<dyn Compute>) {
        self.registry.insert(key, compute);
    }

    /// Point the graph at a mutable env-var map (testable; real deployments
    /// wire `std::env` here).
    pub fn set_env_source(&self, vars: Arc<RwLock<BTreeMap<String, String>>>) {
        *self.env_factory.write().unwrap() = Some(Arc::new(move || {
            Box::new(crate::EnvTrackerImpl { vars: vars.clone() })
        }));
    }

    /// Point the graph at a mutable config map.
    pub fn set_config_source(&self, values: Arc<RwLock<BTreeMap<String, serde_json::Value>>>) {
        *self.config_factory.write().unwrap() = Some(Arc::new(move || {
            Box::new(crate::ConfigTrackerImpl {
                values: values.clone(),
            })
        }));
    }

    /// Override the file-system tracker factory (defaults to real FS reads).
    pub fn set_fs_factory(&self, f: FsFactory) {
        *self.fs_factory.write().unwrap() = Some(f);
    }

    /// Cancel the current build window: sets the token (so `get` returns
    /// `GRAPH-CANCEL` until it is reset) and clears memoized values computed in
    /// the cancelled window — the dev server never serves stale results (§5.6).
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.nodes.clear();
        self.epochs.clear();
    }

    /// Get (or compute) a node. `version` is an epoch counter bumped by invalidation;
    /// nodes computed in an older epoch are recomputed.
    pub fn get(&self, key: NodeKey, compute: &dyn Compute) -> Result<Arc<NodeOutput>> {
        if self.cancel.is_cancelled() {
            return Err(err(
                "GRAPH-CANCEL",
                "get rejected: the current build window was cancelled; reset the CancelToken to continue",
            ));
        }
        if let Some(out) = self.fresh(&key) {
            return Ok(out);
        }
        // Single flight: join an in-flight computation or become its owner.
        let task = match self.in_flight.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(o) => {
                let t: Arc<Task> = o.get().clone();
                drop(o);
                let mut guard = t.state.lock().unwrap();
                loop {
                    match &*guard {
                        TaskState::Running => guard = t.cv.wait(guard).unwrap(),
                        TaskState::Done(res) => {
                            let r = clone_result(res);
                            drop(guard);
                            return r;
                        }
                    }
                }
            }
            dashmap::mapref::entry::Entry::Vacant(v) => {
                // Another thread may have finished between our memo check and
                // this insert — re-check before claiming ownership.
                if let Some(out) = self.fresh(&key) {
                    drop(v);
                    return Ok(out);
                }
                let t = Arc::new(Task {
                    state: Mutex::new(TaskState::Running),
                    cv: Condvar::new(),
                });
                v.insert(t.clone());
                t
            }
        };
        let result = self.compute_and_memoize(&key, compute);
        {
            let mut guard = task.state.lock().unwrap();
            *guard = TaskState::Done(Arc::new(clone_result(&result)));
            task.cv.notify_all();
        }
        self.in_flight.remove(&key);
        result
    }

    /// Run independent node computations in parallel on the rayon pool (§5.6:
    /// CPU nodes on a work-stealing pool). Shared dependencies dedupe through
    /// the single-flight path.
    pub fn get_many(
        &self,
        jobs: Vec<(NodeKey, Arc<dyn Compute>)>,
    ) -> Vec<(NodeKey, Result<Arc<NodeOutput>>)> {
        use rayon::prelude::*;
        jobs.into_par_iter()
            .map(|(k, c)| {
                let r = self.get(k.clone(), c.as_ref());
                (k, r)
            })
            .collect()
    }

    /// Invalidate nodes whose recorded inputs changed; returns the invalidated keys.
    ///
    /// Semantics (§5.1 + §5.6):
    /// - Seeds (nodes that observed a changed input) are always reported, even
    ///   when early cutoff preserved their output — the caller asked about them.
    /// - Transitive dependents are revalidated with early cutoff: a node whose
    ///   recomputed output hash is unchanged keeps its dependents memoized and
    ///   is *not* reported.
    /// - Nodes with no registered Compute are dropped from the memo (reported).
    /// - The version epoch always bumps. Nodes untouched by this invalidation
    ///   are re-stamped only when they have recorded deps that prove they did
    ///   not observe any changed input (and were fresh before the bump). A node
    ///   with **no recorded deps** cannot prove what it observed, so it stays
    ///   stale and recomputes on the next `get` — the §5.6 "never serve stale"
    ///   safety net for computations that raced an invalidation.
    pub fn invalidate(&self, changed: &[Input]) -> Vec<NodeKey> {
        let new_epoch = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        let prev_epoch = new_epoch - 1;

        // 1. Seeds: nodes that observed one of the changed inputs.
        let mut seeds: BTreeSet<NodeKey> = BTreeSet::new();
        for inp in changed {
            match inp {
                Input::Node(k, _) => {
                    seeds.insert(k.clone());
                    if let Some(rdeps) = self.reverse_deps.get(k) {
                        seeds.extend(rdeps.value().iter().cloned());
                    }
                }
                other => {
                    if let Some(set) = self.input_index.get(&InputId::of(other)) {
                        seeds.extend(set.value().iter().cloned());
                    }
                }
            }
        }

        // Reported set: seeds plus everything that actually lost or changed its
        // memo. Early-cut survivors outside the seed set are excluded.
        let mut invalidated: BTreeSet<NodeKey> = seeds.clone();
        let mut closure: BTreeSet<NodeKey> = BTreeSet::new();

        if !seeds.is_empty() {
            // 2. Transitive dependents (reverse-dep walk).
            let mut queue: Vec<NodeKey> = seeds.iter().cloned().collect();
            while let Some(k) = queue.pop() {
                if !closure.insert(k.clone()) {
                    continue;
                }
                if let Some(rdeps) = self.reverse_deps.get(&k) {
                    for d in rdeps.value() {
                        queue.push(d.clone());
                    }
                }
            }

            // 3. Eager revalidation in topological order (dependencies first)
            //    with early cutoff: a survivor spares its dependents.
            let order = topo_order(&closure, &self.nodes);
            let mut status: BTreeMap<NodeKey, Status> = closure
                .iter()
                .map(|k| {
                    (
                        k.clone(),
                        if seeds.contains(k) {
                            Status::RecheckNeeded
                        } else {
                            Status::Unchecked
                        },
                    )
                })
                .collect();
            for k in &order {
                let st = match status.get(k) {
                    Some(Status::RecheckNeeded) => self.recheck(k),
                    _ => Status::Survivor,
                };
                if let Some(rdeps) = self.reverse_deps.get(k) {
                    for d in rdeps.value() {
                        if !closure.contains(d) {
                            continue;
                        }
                        match st {
                            Status::Survivor => {
                                status.entry(d.clone()).or_insert(Status::Survivor);
                            }
                            Status::RecomputedNew | Status::Invalidated => {
                                status.insert(d.clone(), Status::RecheckNeeded);
                            }
                            Status::Unchecked | Status::RecheckNeeded => {}
                        }
                    }
                }
                status.insert(k.clone(), st);
            }

            // 4. Apply: drop invalid memos, re-stamp survivors to the new epoch.
            for (k, st) in &status {
                match st {
                    Status::Invalidated => {
                        self.nodes.remove(k);
                        self.epochs.remove(k);
                        invalidated.insert(k.clone());
                    }
                    Status::Survivor => {
                        self.epochs.insert(k.clone(), new_epoch);
                    }
                    Status::RecomputedNew => {
                        self.epochs.insert(k.clone(), new_epoch);
                        invalidated.insert(k.clone());
                    }
                    Status::Unchecked | Status::RecheckNeeded => {
                        // Unreachable in a consistent walk; treat conservatively.
                        self.nodes.remove(k);
                        self.epochs.remove(k);
                        invalidated.insert(k.clone());
                    }
                }
            }
        }

        // 5. Re-stamp nodes untouched by this invalidation so the epoch bump
        //    does not flush them — but only when their recorded deps prove they
        //    observed none of the changed inputs (non-closure ⇒ true, since the
        //    input index produced exactly the matching seeds) and they were
        //    fresh before this bump. Dep-free nodes and already-stale nodes keep
        //    their old epoch and recompute on the next `get` (§5.6 safety net).
        for kv in self.nodes.iter() {
            let k = kv.key();
            if closure.contains(k) || kv.value().deps.is_empty() {
                continue;
            }
            if self.epochs.get(k).map(|e| *e.value()) == Some(prev_epoch) {
                self.epochs.insert(k.clone(), new_epoch);
            }
        }

        invalidated.into_iter().collect()
    }

    /// Current memoized output, if fresh.
    pub fn peek(&self, key: &NodeKey) -> Option<Arc<NodeOutput>> {
        self.fresh(key)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    // -----------------------------------------------------------------
    // internals
    // -----------------------------------------------------------------

    fn fresh(&self, key: &NodeKey) -> Option<Arc<NodeOutput>> {
        let epoch = self.version.load(Ordering::SeqCst);
        let e = *self.epochs.get(key)?;
        if e != epoch {
            return None;
        }
        self.nodes.get(key).map(|o| Arc::new(o.value().clone()))
    }

    fn recheck(&self, key: &NodeKey) -> Status {
        let old_hash = self.nodes.get(key).map(|o| o.value().hash);
        let registered = self.registry.get(key).map(|c| c.value().clone());
        match registered {
            Some(compute) => match self.compute_and_memoize(key, compute.as_ref()) {
                Ok(new_out) => match old_hash {
                    // Early cutoff: same output hash → memo refreshed (new deps),
                    // dependents keep theirs.
                    Some(h) if h == new_out.hash => Status::Survivor,
                    _ => Status::RecomputedNew,
                },
                Err(_) => Status::Invalidated,
            },
            None => Status::Invalidated,
        }
    }

    fn compute_and_memoize(&self, key: &NodeKey, compute: &dyn Compute) -> Result<Arc<NodeOutput>> {
        // Stamp the epoch observed *before* computing: if an invalidation bumps
        // the epoch mid-compute, the result is stale by definition (§5.6).
        let epoch_at_start = self.version.load(Ordering::SeqCst);
        let deps_cell = RefCell::new(BTreeSet::new());
        let result = {
            let mut fs = self.make_fs();
            let mut env = self.make_env();
            let mut config = self.make_config();
            let mut rec_fs = RecFs {
                inner: fs.as_mut(),
                deps: &deps_cell,
            };
            let mut rec_env = RecEnv {
                inner: env.as_mut(),
                deps: &deps_cell,
            };
            let mut rec_cfg = RecCfg {
                inner: config.as_mut(),
                deps: &deps_cell,
            };
            let mut ctx = ComputeCtx {
                fs: &mut rec_fs,
                env: &mut rec_env,
                config: &mut rec_cfg,
                node_outputs: &self.nodes,
                cancel: &self.cancel,
                graph: self,
                deps: &deps_cell,
            };
            COMPUTE_STACK.with(|s| s.borrow_mut().push(key.clone()));
            let r = compute.compute(&mut ctx);
            COMPUTE_STACK.with(|s| {
                s.borrow_mut().pop();
            });
            r
        };
        if self.cancel.is_cancelled() {
            return Err(err(
                "GRAPH-CANCEL",
                "computation finished in a cancelled window; result discarded",
            ));
        }
        let out = result?;
        let deps = deps_cell.into_inner();
        for d in &deps {
            match d {
                Input::Node(k, _) => {
                    self.reverse_deps
                        .entry(k.clone())
                        .or_default()
                        .insert(key.clone());
                }
                other => {
                    self.input_index
                        .entry(InputId::of(other))
                        .or_default()
                        .insert(key.clone());
                }
            }
        }
        let out = NodeOutput {
            hash: out.hash,
            artifact: out.artifact,
            deps,
        };
        self.nodes.insert(key.clone(), out.clone());
        self.epochs.insert(key.clone(), epoch_at_start);
        Ok(Arc::new(out))
    }

    /// `ctx.dep(key)` — memo hit records the edge and returns; otherwise the
    /// registered compute demand-computes the node (§5.1 demand-driven model).
    pub(crate) fn dep_from_ctx(&self, ctx: &ComputeCtx, key: &NodeKey) -> Result<Arc<NodeOutput>> {
        let cycle = COMPUTE_STACK.with(|s| s.borrow().iter().any(|k| k == key));
        if cycle {
            return Err(err(
                "GRAPH-CYCLE",
                format!("dependency cycle detected at node `{}`", key.0),
            ));
        }
        if let Some(out) = self.fresh(key) {
            ctx.deps
                .borrow_mut()
                .insert(Input::Node(key.clone(), out.hash));
            return Ok(out);
        }
        let compute = self
            .registry
            .get(key)
            .map(|c| c.value().clone())
            .ok_or_else(|| {
                err(
                    "GRAPH-NO-COMPUTE",
                    format!(
                        "node `{}` is not memoized and has no registered Compute",
                        key.0
                    ),
                )
            })?;
        let out = self.get(key.clone(), compute.as_ref())?;
        ctx.deps
            .borrow_mut()
            .insert(Input::Node(key.clone(), out.hash));
        Ok(out)
    }

    fn make_fs(&self) -> Box<dyn FileTracker> {
        self.fs_factory
            .read()
            .unwrap()
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| Box::new(crate::FsTrackerImpl::new()))
    }

    fn make_env(&self) -> Box<dyn EnvTracker> {
        self.env_factory
            .read()
            .unwrap()
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| Box::new(crate::EnvTrackerImpl::new(Default::default())))
    }

    fn make_config(&self) -> Box<dyn ConfigTracker> {
        self.config_factory
            .read()
            .unwrap()
            .as_ref()
            .map(|f| f())
            .unwrap_or_else(|| Box::new(crate::ConfigTrackerImpl::new(Default::default())))
    }
}

/// Kahn topological order over intra-closure node-dep edges, deterministic via
/// BTreeMaps. Nodes whose memo is missing contribute no edges and are ordered
/// by key.
fn topo_order(closure: &BTreeSet<NodeKey>, nodes: &DashMap<NodeKey, NodeOutput>) -> Vec<NodeKey> {
    let mut indegree: BTreeMap<NodeKey, usize> = closure.iter().map(|k| (k.clone(), 0)).collect();
    let mut dependents: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    for k in closure {
        if let Some(out) = nodes.get(k) {
            for d in out.value().deps.iter() {
                if let Input::Node(dep, _) = d {
                    if closure.contains(dep) && dep != k {
                        dependents.entry(dep.clone()).or_default().push(k.clone());
                        *indegree.get_mut(k).unwrap() += 1;
                    }
                }
            }
        }
    }
    let mut ready: BTreeSet<NodeKey> = indegree
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut order = Vec::with_capacity(closure.len());
    while let Some(k) = ready.pop_first() {
        order.push(k.clone());
        if let Some(deps) = dependents.get(&k) {
            for m in deps.clone() {
                let e = indegree.get_mut(&m).unwrap();
                *e -= 1;
                if *e == 0 {
                    ready.insert(m);
                }
            }
        }
    }
    if order.len() < closure.len() {
        // Residual cycle in estimated edges (stale reverse deps) — conservative
        // key-ordered fallback.
        for k in closure {
            if !order.contains(k) {
                order.push(k.clone());
            }
        }
    }
    order
}
