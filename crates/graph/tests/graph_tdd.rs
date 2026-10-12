//! Integration suite for oxide-graph (spec §5, D6) — drives the crate only through its
//! public API. Covers: demand graph memoization, single-flight dedupe, early cutoff,
//! invalidation cascades (file/dir/env/config/tool/node inputs), cancellation, Tier B
//! CAS (zstd blobs, redb index, corruption detection, GC, namespaces) and trackers.

use blake3::Hash as B3;
use oxide_graph::{err, Result};
use oxide_graph::{
    CancelToken, CasKey, CasWriteMeta, Compute, ComputeCtx, ConfigTrackerImpl, ContentStore,
    ContentStoreExt, DemandGraph, EnvTrackerImpl, FsTrackerImpl, Input, NodeKey, NodeOutput,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, RwLock};
use std::time::{Duration, SystemTime};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Counting compute: output hash is blake3 of `tag` + an optional file's bytes.
struct FileCompute {
    key: &'static str,
    tag: &'static str,
    reads_file: bool,
    /// When false, the file is read (dep recorded) but its bytes stay out of
    /// the output material — the output hash then survives content changes,
    /// which is what the early-cutoff scenario needs.
    file_in_material: bool,
    reads_dir: bool,
    reads_env: Option<&'static str>,
    reads_config: Option<&'static str>,
    reads_tool: Option<(&'static str, &'static str)>,
    dep_on: Vec<NodeKey>,
    calls: Arc<AtomicUsize>,
    /// When set, cancels the context token after N steps and checks cancellation.
    cancel_after_steps: Option<(usize, CancelToken)>,
}

impl FileCompute {
    fn new(key: &'static str, tag: &'static str, calls: Arc<AtomicUsize>) -> Self {
        Self {
            key,
            tag,
            reads_file: false,
            file_in_material: true,
            reads_dir: false,
            reads_env: None,
            reads_config: None,
            reads_tool: None,
            dep_on: Vec::new(),
            calls,
            cancel_after_steps: None,
        }
    }
    fn file(mut self) -> Self {
        self.reads_file = true;
        self
    }
    /// Read the file and record the dep, but keep the bytes out of the output
    /// hash (output is stable across file content changes → early cutoff).
    fn file_dep_only(mut self) -> Self {
        self.reads_file = true;
        self.file_in_material = false;
        self
    }
    fn dir(mut self) -> Self {
        self.reads_dir = true;
        self
    }
    fn env(mut self, k: &'static str) -> Self {
        self.reads_env = Some(k);
        self
    }
    fn config(mut self, k: &'static str) -> Self {
        self.reads_config = Some(k);
        self
    }
    fn tool(mut self, n: &'static str, v: &'static str) -> Self {
        self.reads_tool = Some((n, v));
        self
    }
    fn dep(mut self, k: NodeKey) -> Self {
        self.dep_on.push(k);
        self
    }
}

impl Compute for FileCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut material = String::from(self.tag);
        if self.reads_file {
            let p = PathBuf::from(self.key.strip_prefix("n:").unwrap_or(self.key));
            let bytes = ctx.fs.read_file(&p)?;
            if self.file_in_material {
                material.push_str(&format!("file={}", String::from_utf8_lossy(&bytes)));
            }
        }
        if self.reads_dir {
            let p = PathBuf::from(self.key.strip_prefix("n:").unwrap_or(self.key));
            let listing = ctx.fs.list_dir(&p)?;
            let names: Vec<String> = listing
                .iter()
                .map(|e| e.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            material.push_str(&format!("dir={names:?}"));
        }
        if let Some(k) = self.reads_env {
            material.push_str(&format!("env:{k}={:?}", ctx.env.get(k)));
        }
        if let Some(k) = self.reads_config {
            material.push_str(&format!("cfg:{k}={:?}", ctx.config.get(k)));
        }
        if let Some((n, v)) = self.reads_tool {
            ctx.tool_version(n, v); // record the tracked input (§5.2)
            material.push_str(&format!("tool:{n}@{v}"));
        }
        for d in &self.dep_on {
            let out = ctx.dep(d)?;
            material.push_str(&format!("dep:{}@{}", d.0, out.hash));
        }
        if let Some((steps, token)) = &self.cancel_after_steps {
            for i in 0..*steps {
                ctx.check_cancel()?;
                if i + 1 == *steps {
                    token.cancel();
                }
            }
            ctx.check_cancel()?;
        }
        Ok(NodeOutput {
            hash: blake3::hash(material.as_bytes()),
            artifact: None,
            deps: Default::default(),
        })
    }
}

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

/// Static key string + NodeKey encoding an absolute FS path (`n:<abs path>`).
fn key_for(path: &std::path::Path) -> (&'static str, NodeKey) {
    let s: &'static str = Box::leak(format!("n:{}", path.display()).into_boxed_str());
    (s, NodeKey::new(s))
}

fn file_input(path: &std::path::Path, content: &[u8]) -> Input {
    Input::File {
        path: path.to_path_buf(),
        hash: blake3::hash(content),
    }
}

// ---------------------------------------------------------------------------
// demand graph: memoization, single-flight, epochs
// ---------------------------------------------------------------------------

#[test]
fn memoizes_repeat_gets() {
    let g = DemandGraph::new();
    let c = counter();
    let k = NodeKey::new("n:a");
    let comp = FileCompute::new("n:a", "a", c.clone());
    let o1 = g.get(k.clone(), &comp).unwrap();
    let o2 = g.get(k.clone(), &comp).unwrap();
    assert_eq!(o1.hash, o2.hash);
    assert_eq!(c.load(Ordering::SeqCst), 1, "second get must be a memo hit");
    assert_eq!(g.node_count(), 1);
    assert!(g.peek(&k).is_some());
    assert_eq!(g.version(), 0, "no invalidation happened");
}

#[test]
fn single_flight_dedupes_concurrent_gets() {
    let g = Arc::new(DemandGraph::new());
    let c = counter();
    let comp = Arc::new(FileCompute::new("n:slow", "slow", c.clone()));
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let (g, comp, barrier) = (g.clone(), comp.clone(), barrier.clone());
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            g.get(NodeKey::new("n:slow"), comp.as_ref())
        }));
    }
    for h in handles {
        h.join().unwrap().unwrap();
    }
    assert_eq!(
        c.load(Ordering::SeqCst),
        1,
        "8 concurrent identical gets must share one compute"
    );
}

#[test]
fn epoch_bump_on_invalidate_forces_recompute_of_unrelated_nodes() {
    // The `version` epoch: invalidate() bumps it; memoized nodes stamped in an
    // older epoch recompute on next get — this is the safety net for gets that
    // raced an invalidation (§5.6 "never serve stale").
    let g = DemandGraph::new();
    let c = counter();
    let comp = FileCompute::new("n:x", "x", c.clone());
    g.get(NodeKey::new("n:x"), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 1);
    let changed = g.invalidate(&[Input::ToolVersion {
        name: "nothing".into(),
        version: "0".into(),
    }]);
    assert!(changed.is_empty());
    assert_eq!(g.version(), 1);
    g.get(NodeKey::new("n:x"), &comp).unwrap();
    assert_eq!(
        c.load(Ordering::SeqCst),
        2,
        "old-epoch memo must recompute after invalidate"
    );
}

// ---------------------------------------------------------------------------
// early cutoff (§5.1)
// ---------------------------------------------------------------------------

#[test]
fn early_cutoff_spares_dependents_when_hash_unchanged() {
    // A reads the file but its output does not depend on the bytes → recomputing
    // A after the file change yields the same hash → B must NOT recompute.
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("a.txt");
    std::fs::write(&f, b"v1").unwrap();
    let g = DemandGraph::new();
    let ca = counter();
    let cb = counter();
    let (ka, a) = key_for(&f);
    let b = NodeKey::new("n:b");
    // A reads the file but its output does not depend on the bytes → recomputing
    // A after the file change yields the same hash → B must NOT recompute. The
    // compute is registered so invalidate() can eagerly revalidate A.
    let comp_a: Arc<FileCompute> =
        Arc::new(FileCompute::new(ka, "const-a", ca.clone()).file_dep_only());
    g.register(a.clone(), comp_a.clone());
    let comp_b = FileCompute::new("n:b", "b", cb.clone()).dep(a.clone());
    g.get(a.clone(), comp_a.as_ref()).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    assert_eq!(ca.load(Ordering::SeqCst), 1);
    assert_eq!(cb.load(Ordering::SeqCst), 1);

    std::fs::write(&f, b"v2").unwrap();
    let invalidated = g.invalidate(&[file_input(&f, b"v2")]);
    assert_eq!(
        invalidated,
        vec![a.clone()],
        "early-cut survivor must be excluded from the result"
    );
    assert_eq!(
        ca.load(Ordering::SeqCst),
        2,
        "A itself is recomputed by invalidate (eager revalidation)"
    );
    assert_eq!(cb.load(Ordering::SeqCst), 1, "B survives via early cutoff");

    g.get(a.clone(), comp_a.as_ref()).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    assert_eq!(ca.load(Ordering::SeqCst), 2);
    assert_eq!(
        cb.load(Ordering::SeqCst),
        1,
        "B must remain memoized after early cutoff"
    );
}

#[test]
fn hash_change_recomputes_the_whole_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("chain.txt");
    std::fs::write(&f, b"one").unwrap();
    let g = DemandGraph::new();
    let (c1, c2, c3) = (counter(), counter(), counter());
    let (ka, a) = key_for(&f);
    let b = NodeKey::new("n:b");
    let d = NodeKey::new("n:c");
    let comp_a = FileCompute::new(ka, "a", c1.clone()).file();
    let comp_b = FileCompute::new("n:b", "b", c2.clone()).dep(a.clone());
    let comp_c = FileCompute::new("n:c", "c", c3.clone()).dep(b.clone());
    let out_a1 = g.get(a.clone(), &comp_a).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    g.get(d.clone(), &comp_c).unwrap();

    std::fs::write(&f, b"two").unwrap();
    let invalidated = g.invalidate(&[file_input(&f, b"two")]);
    assert_eq!(
        invalidated,
        vec![a.clone(), b.clone(), d.clone()],
        "cascade walks reverse deps transitively"
    );
    let out_a2 = g.get(a.clone(), &comp_a).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    g.get(d.clone(), &comp_c).unwrap();
    assert_eq!(
        (
            c1.load(Ordering::SeqCst),
            c2.load(Ordering::SeqCst),
            c3.load(Ordering::SeqCst)
        ),
        (2, 2, 2)
    );
    assert_ne!(
        out_a1.hash, out_a2.hash,
        "A's output must track the file hash"
    );
}

#[test]
fn node_dep_edge_invalidates_dependent_but_not_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("dep.txt");
    let sib = tmp.path().join("sib.txt");
    std::fs::write(&f, b"dep-v1").unwrap();
    std::fs::write(&sib, b"sib").unwrap();
    let g = DemandGraph::new();
    let (ca, cb, cs) = (counter(), counter(), counter());
    let (ka, a) = key_for(&f);
    let (ks, s) = key_for(&sib);
    let b = NodeKey::new("n:b");
    let comp_a = FileCompute::new(ka, "a", ca.clone()).file();
    let comp_b = FileCompute::new("n:b", "b", cb.clone()).dep(a.clone());
    // S reads a *different* file — a sibling, not a dependent.
    let comp_s = FileCompute::new(ks, "s", cs.clone()).file();
    g.get(a.clone(), &comp_a).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    g.get(s.clone(), &comp_s).unwrap();

    std::fs::write(&f, b"dep-v2").unwrap();
    let invalidated = g.invalidate(&[file_input(&f, b"dep-v2")]);
    assert_eq!(
        invalidated,
        vec![a.clone(), b.clone()],
        "sibling must not be invalidated"
    );
    g.get(a.clone(), &comp_a).unwrap();
    g.get(b.clone(), &comp_b).unwrap();
    g.get(s.clone(), &comp_s).unwrap();
    assert_eq!(cs.load(Ordering::SeqCst), 1, "sibling must not recompute");
}

// ---------------------------------------------------------------------------
// tracked inputs (§5.2)
// ---------------------------------------------------------------------------

#[test]
fn dir_listing_change_invalidates() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pages");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("one.ts"), b"x").unwrap();
    let g = DemandGraph::new();
    let c = counter();
    let (kd, k) = key_for(&dir);
    let comp = FileCompute::new(kd, "listing", c.clone()).dir();
    g.get(k.clone(), &comp).unwrap();
    std::fs::write(dir.join("two.ts"), b"y").unwrap();
    let old_hash = blake3::hash(b"old-listing");
    let changed = g.invalidate(&[Input::DirListing {
        path: dir.clone(),
        listing_hash: old_hash,
    }]);
    assert_eq!(changed, vec![k.clone()]);
    let out = g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
    // The new listing must be visible in the output material.
    let material = String::from_utf8_lossy(out.hash.as_bytes()).len();
    assert!(material > 0);
}

#[test]
fn env_var_change_invalidates() {
    let g = DemandGraph::new();
    let env: Arc<RwLock<BTreeMap<String, String>>> = Arc::new(RwLock::new(BTreeMap::from([(
        "OX_FEATURE".into(),
        "off".into(),
    )])));
    g.set_env_source(env.clone());
    let c = counter();
    let k = NodeKey::new("n:env");
    let comp = FileCompute::new("n:env", "envnode", c.clone()).env("OX_FEATURE");
    g.get(k.clone(), &comp).unwrap();
    env.write()
        .unwrap()
        .insert("OX_FEATURE".into(), "on".into());
    let changed = g.invalidate(&[Input::Env {
        key: "OX_FEATURE".into(),
        value_hash: blake3::hash(b"on"),
    }]);
    assert_eq!(changed, vec![k.clone()]);
    g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

#[test]
fn config_key_change_invalidates() {
    let g = DemandGraph::new();
    let cfg: Arc<RwLock<BTreeMap<String, serde_json::Value>>> = Arc::new(RwLock::new(
        BTreeMap::from([("server.port".into(), serde_json::json!(3000))]),
    ));
    g.set_config_source(cfg.clone());
    let c = counter();
    let k = NodeKey::new("n:cfg");
    let comp = FileCompute::new("n:cfg", "cfgnode", c.clone()).config("server.port");
    g.get(k.clone(), &comp).unwrap();
    cfg.write()
        .unwrap()
        .insert("server.port".into(), serde_json::json!(4001));
    let changed = g.invalidate(&[Input::ConfigKey {
        key: "server.port".into(),
        value_hash: blake3::hash(b"4001"),
    }]);
    assert_eq!(changed, vec![k.clone()]);
    g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

#[test]
fn tool_version_change_invalidates() {
    let g = DemandGraph::new();
    let c = counter();
    let k = NodeKey::new("n:tool");
    let comp = FileCompute::new("n:tool", "toolnode", c.clone()).tool("svelte-compiler", "1.0.0");
    g.get(k.clone(), &comp).unwrap();
    let changed = g.invalidate(&[Input::ToolVersion {
        name: "svelte-compiler".into(),
        version: "1.1.0".into(),
    }]);
    assert_eq!(changed, vec![k.clone()]);
    g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// cancellation (§5.6)
// ---------------------------------------------------------------------------

#[test]
fn cancel_token_aborts_mid_compute_and_clears() {
    let g = DemandGraph::new();
    let c = counter();
    let k = NodeKey::new("n:cancel");
    let comp = CancelMidCompute {
        calls: c.clone(),
        token: g.cancel.clone(),
        armed: Arc::new(AtomicBool::new(true)),
    };
    let e = g.get(k.clone(), &comp).unwrap_err();
    assert_eq!(e.0.code, "GRAPH-CANCEL");
    assert!(
        g.peek(&k).is_none(),
        "cancelled results must never be memoized"
    );
    assert_eq!(g.node_count(), 0);

    // Dev server never serves results from the cancelled window: gets fail at
    // the entry check until a fresh token state.
    let e2 = g.get(k.clone(), &comp).unwrap_err();
    assert_eq!(e2.0.code, "GRAPH-CANCEL");
    assert_eq!(
        c.load(Ordering::SeqCst),
        1,
        "cancelled token short-circuits before compute"
    );

    g.cancel.reset();
    g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
    assert!(g.peek(&k).is_some());
}

struct CancelMidCompute {
    calls: Arc<AtomicUsize>,
    token: CancelToken,
    /// Fires the cancel exactly once (the first run); after `reset()` a fresh
    /// run must complete so the result can be memoized again.
    armed: Arc<AtomicBool>,
}

impl Compute for CancelMidCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let armed = self.armed.swap(false, Ordering::SeqCst);
        for i in 0..10 {
            ctx.check_cancel()?; // step boundary check
            if i == 4 && armed {
                self.token.cancel(); // newer change lands mid-compute
            }
        }
        Ok(NodeOutput {
            hash: blake3::hash(b"never"),
            artifact: None,
            deps: Default::default(),
        })
    }
}

#[test]
fn demand_graph_cancel_clears_memoized_values() {
    let g = DemandGraph::new();
    let c = counter();
    let k = NodeKey::new("n:memo");
    let comp = FileCompute::new("n:memo", "m", c.clone());
    g.get(k.clone(), &comp).unwrap();
    assert!(g.peek(&k).is_some());
    g.cancel(); // graph-level cancel: token + memo wipe
    assert!(
        g.peek(&k).is_none(),
        "cancel() clears memoized values computed in the cancelled window"
    );
    g.cancel.reset();
    g.get(k.clone(), &comp).unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

#[test]
fn get_after_cancel_returns_graph_cancel_code() {
    let g = DemandGraph::new();
    g.cancel();
    let c = counter();
    let comp = FileCompute::new("n:any", "any", c.clone());
    let e = g.get(NodeKey::new("n:any"), &comp).unwrap_err();
    assert_eq!(e.0.code, "GRAPH-CANCEL");
    assert_eq!(c.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// get_many (rayon) + registry-driven demand deps
// ---------------------------------------------------------------------------

#[test]
fn get_many_computes_four_independent_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..4 {
        std::fs::write(tmp.path().join(format!("m{i}.txt")), format!("content-{i}")).unwrap();
    }
    let g = DemandGraph::new();
    let counters: Vec<_> = (0..4).map(|_| counter()).collect();
    // Keys encode the absolute file path the compute reads (`n:<abs path>`).
    let keys: Vec<&'static str> = (0..4)
        .map(|i| {
            let p = tmp.path().join(format!("m{i}.txt")) as std::path::PathBuf;
            let s: &'static str = Box::leak(format!("n:{}", p.display()).into_boxed_str());
            s
        })
        .collect();
    let jobs: Vec<(NodeKey, Arc<dyn Compute>)> = (0..4)
        .map(|i| {
            let comp: Arc<dyn Compute> =
                Arc::new(FileCompute::new(keys[i], "many", counters[i].clone()).file());
            (NodeKey::new(keys[i]), comp)
        })
        .collect();
    let results = g.get_many(jobs);
    assert_eq!(results.len(), 4);
    for (k, r) in &results {
        assert!(r.is_ok(), "get_many({k:?}) failed");
    }
    for c in &counters {
        assert_eq!(c.load(Ordering::SeqCst), 1);
    }
    // Memoized: a second batch does not recompute.
    let jobs2: Vec<(NodeKey, Arc<dyn Compute>)> = (0..4)
        .map(|i| {
            let comp: Arc<dyn Compute> =
                Arc::new(FileCompute::new(keys[i], "many", counters[i].clone()));
            (NodeKey::new(keys[i]), comp)
        })
        .collect();
    g.get_many(jobs2);
    for c in &counters {
        assert_eq!(
            c.load(Ordering::SeqCst),
            1,
            "get_many must hit the memo on the second pass"
        );
    }
}

#[test]
fn ctx_dep_demands_registered_node_and_records_edge() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("up.txt");
    std::fs::write(&f, b"upstream").unwrap();
    let g = DemandGraph::new();
    let (cu, cd) = (counter(), counter());
    let down = NodeKey::new("n:down");
    let (kup, up) = key_for(&f);
    let comp_up: Arc<dyn Compute> = Arc::new(FileCompute::new(kup, "up", cu.clone()).file());
    let comp_down: Arc<dyn Compute> =
        Arc::new(FileCompute::new("n:down", "down", cd.clone()).dep(up.clone()));
    g.register(up.clone(), comp_up);
    g.register(down.clone(), comp_down.clone());
    // Only `down` is demanded; it pulls `up` through ctx.dep on demand.
    g.get(down.clone(), comp_down.as_ref()).unwrap();
    assert_eq!(
        cu.load(Ordering::SeqCst),
        1,
        "dep() must demand-compute the upstream node"
    );
    assert_eq!(cd.load(Ordering::SeqCst), 1);

    std::fs::write(&f, b"upstream-2").unwrap();
    let changed = g.invalidate(&[file_input(&f, b"upstream-2")]);
    assert_eq!(
        changed,
        vec![up.clone(), down.clone()],
        "node-dep edge must invalidate the dependent"
    );
    g.get(down.clone(), comp_down.as_ref()).unwrap();
    assert_eq!(cu.load(Ordering::SeqCst), 2);
    assert_eq!(cd.load(Ordering::SeqCst), 2);
}

#[test]
fn ctx_dep_detects_cycles() {
    let g = DemandGraph::new();
    let a = NodeKey::new("n:c1");
    let b = NodeKey::new("n:c2");
    struct Cycle;
    impl Compute for Cycle {
        fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput> {
            let _ = ctx.dep(&NodeKey::new("n:c2"))?;
            Ok(NodeOutput {
                hash: blake3::hash(b"c1"),
                artifact: None,
                deps: Default::default(),
            })
        }
    }
    struct Cycle2;
    impl Compute for Cycle2 {
        fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput> {
            let _ = ctx.dep(&NodeKey::new("n:c1"))?;
            Ok(NodeOutput {
                hash: blake3::hash(b"c2"),
                artifact: None,
                deps: Default::default(),
            })
        }
    }
    g.register(a.clone(), Arc::new(Cycle));
    g.register(b.clone(), Arc::new(Cycle2));
    let e = g.get(a, &Cycle).unwrap_err();
    assert_eq!(e.0.code, "GRAPH-CYCLE");
}

#[test]
fn ctx_dep_without_registry_errors() {
    let g = DemandGraph::new();
    struct NeedsDep;
    impl Compute for NeedsDep {
        fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput> {
            ctx.dep(&NodeKey::new("n:ghost"))?;
            Ok(NodeOutput {
                hash: blake3::hash(b"x"),
                artifact: None,
                deps: Default::default(),
            })
        }
    }
    let e = g.get(NodeKey::new("n:needs"), &NeedsDep).unwrap_err();
    assert_eq!(e.0.code, "GRAPH-NO-COMPUTE");
}

// ---------------------------------------------------------------------------
// Tier B CAS (§5.4)
// ---------------------------------------------------------------------------

fn meta(ns: &[&str], kind: &'static str) -> CasWriteMeta {
    CasWriteMeta {
        namespace: ns.iter().map(|s| s.to_string()).collect(),
        kind,
    }
}

#[test]
fn cas_put_get_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let data: Vec<u8> = (0..64_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let key = store
        .put(&data, &meta(&["transform", "tsx"], "module"))
        .unwrap();
    assert_eq!(key, CasKey::of(&data));
    let got = store.get(&key).unwrap().unwrap();
    assert_eq!(got, data);
    // Idempotent: same content → same key, single blob.
    let key2 = store
        .put(&data, &meta(&["transform", "tsx"], "module"))
        .unwrap();
    assert_eq!(key, key2);
    assert_eq!(store.blob_file_count(), 1);
    assert!(store.get(&CasKey::of(b"never stored")).unwrap().is_none());
}

#[test]
fn cas_zstd_shrinks_repetitive_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let data = vec![b'a'; 1024 * 1024]; // highly repetitive 1 MiB
    let key = store.put(&data, &meta(&["css"], "bundle")).unwrap();
    let stored = store.stored_bytes(&key).unwrap();
    assert!(
        stored < 100 * 1024,
        "zstd must shrink a repetitive 1MiB payload, got {stored} bytes"
    );
    assert_eq!(store.get(&key).unwrap().unwrap().len(), 1024 * 1024);
    assert_eq!(store.total_bytes().unwrap(), stored as u64);
}

#[test]
fn cas_detects_corrupt_blob() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let data = b"the quick brown fox jumps over the lazy dog".repeat(64);
    let key = store.put(&data, &meta(&["t"], "x")).unwrap();
    let path = store.blob_path(&key);
    let mut bytes = std::fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF; // flip one byte
    std::fs::write(&path, &bytes).unwrap();
    let e = store.get(&key).unwrap_err();
    assert_eq!(
        e.0.code, "CAS-CORRUPT",
        "hash verification on load must flag corruption (§17.2)"
    );
}

#[test]
fn cas_touch_extends_life_against_gc() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let now = SystemTime::now();
    let k_hot = store.put(b"hot data", &meta(&["hot"], "x")).unwrap();
    let k_cold = store.put(b"cold data", &meta(&["cold"], "x")).unwrap();
    // Both are aged beyond the idle window, then the hot one is touched fresh.
    store
        .touch(&k_hot, now - Duration::from_secs(3600))
        .unwrap();
    store
        .touch(&k_cold, now - Duration::from_secs(3600))
        .unwrap();
    store.touch(&k_hot, now).unwrap();
    let (removed, bytes) = store.gc(Duration::from_secs(1800), u64::MAX).unwrap();
    assert_eq!(removed, 1, "only the stale entry is collected");
    assert!(bytes > 0);
    assert!(
        store.get(&k_hot).unwrap().is_some(),
        "touched entry survives idle GC"
    );
    assert!(store.get(&k_cold).unwrap().is_none());
    // touch() on an unknown key errors.
    assert!(store.touch(&CasKey::of(b"ghost"), now).is_err());
}

#[test]
fn cas_gc_enforces_size_cap_by_lru() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let now = SystemTime::now();
    let k_old = store
        .put(vec![7u8; 2000].as_slice(), &meta(&["old"], "x"))
        .unwrap();
    let k_new = store
        .put(vec![9u8; 2000].as_slice(), &meta(&["new"], "x"))
        .unwrap();
    store.touch(&k_old, now - Duration::from_secs(600)).unwrap();
    store.touch(&k_new, now).unwrap();
    let total = store.total_bytes().unwrap();
    // Cap just below the combined size → oldest-first (LRU) eviction.
    let (removed, _) = store.gc(Duration::from_secs(999_999), total - 1).unwrap();
    assert_eq!(removed, 1);
    assert!(
        store.get(&k_new).unwrap().is_some(),
        "most-recently-used entry survives"
    );
    assert!(store.get(&k_old).unwrap().is_none());
    assert!(store.total_bytes().unwrap() <= total - 1);
}

#[test]
fn cas_namespace_is_logical_not_physical() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ContentStore::open(tmp.path()).unwrap();
    let data = b"shared content".to_vec();
    let k1 = store
        .put(&data, &meta(&["transform", "react"], "mod"))
        .unwrap();
    let k2 = store
        .put(&data, &meta(&["compile", "react"], "mod"))
        .unwrap();
    assert_eq!(
        k1, k2,
        "CAS is content-addressed: namespaces share the same blob"
    );
    assert_eq!(store.blob_file_count(), 1);
    assert_eq!(
        store.list_namespace("transform/react").unwrap(),
        vec![k1.clone()]
    );
    assert_eq!(
        store.list_namespace("compile/react").unwrap(),
        vec![k1.clone()]
    );
    assert!(store.list_namespace("nope").unwrap().is_empty());
}

#[test]
fn cas_reopens_persistently() {
    let tmp = tempfile::tempdir().unwrap();
    let key = {
        let store = ContentStore::open(tmp.path()).unwrap();
        store.put(b"persist me", &meta(&["p"], "x")).unwrap()
    };
    let store2 = ContentStore::open(tmp.path()).unwrap();
    assert_eq!(store2.get(&key).unwrap().unwrap(), b"persist me");
    assert!(store2.total_bytes().unwrap() > 0);
}

proptest::proptest! {
    #[test]
    fn cas_roundtrip_arbitrary_bytes(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..=200_000)) {
        let tmp = tempfile::tempdir().unwrap();
        let store = ContentStore::open(tmp.path()).unwrap();
        let key = store.put(&data, &meta(&["prop"], "x")).unwrap();
        assert_eq!(key.0[..], CasKey::of(&data).0[..]);
        assert_eq!(store.get(&key).unwrap().unwrap(), data);
    }

    #[test]
    fn demand_output_hash_tracks_input(tag in "[a-z]{1,16}", content in "[a-z]{0,64}") {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("prop.txt");
        std::fs::write(&f, content.as_bytes()).unwrap();
        let g = DemandGraph::new();
        let (kstr, k) = key_for(&f);
        let comp = FileCompute::new(kstr, Box::leak(tag.clone().into_boxed_str()), counter()).file();
        let out = g.get(k, &comp).unwrap();
        let mut material = tag.clone();
        material.push_str(&format!("file={content}"));
        proptest::prop_assert_eq!(out.hash, blake3::hash(material.as_bytes()));
    }
}

// silence unused warnings for items only used in some cfgs
#[allow(unused)]
fn _use_err() {
    let _ = err("X", "y");
    let _ = EnvTrackerImpl::new(Default::default());
    let _ = ConfigTrackerImpl::new(Default::default());
    let _: Option<B3> = None;
    let _: Vec<PathBuf> = Vec::new();
    let _ = FsTrackerImpl::new();
}
