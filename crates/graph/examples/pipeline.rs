//! The demand-driven incremental engine, walked
//! (`cargo run -p oxide-fw-graph --example pipeline`).
//!
//! `oxide-fw-graph` had no consumer example — this one walks the shape the
//! build (`crates/cli/src/build_graph.rs`) drives in production, shrunk to
//! two nodes over an env source so it stays deterministic and needs no
//! filesystem: a config node whose hash follows `OXIDE_EXAMPLE_MODE`, and a
//! bundle node that demands the config through [`ComputeCtx::dep`]. The
//! walk shows the three halves the engine exists for:
//!
//! 1. demand — `get` computes exactly what the asked-for node needs, in
//!    dependency order, and memoizes;
//! 2. memoization — a re-`get` answers without recomputing (the version
//!    does not move);
//! 3. invalidation — an `Input::Env` change invalidates the node that
//!    observed it AND everything downstream of it, so the next `get`
//!    recomputes both (fresh hashes, same keys).

use oxide_fw_graph::{Compute, ComputeCtx, DemandGraph, Input, NodeKey, NodeOutput};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

fn env_source(mode: &str) -> Arc<RwLock<BTreeMap<String, String>>> {
    let mut vars = BTreeMap::new();
    vars.insert("OXIDE_EXAMPLE_MODE".to_string(), mode.to_string());
    Arc::new(RwLock::new(vars))
}

/// The config node: reads one env var — the read is recorded as an
/// `Input::Env` edge automatically, so an env change can invalidate it.
struct ConfigCompute;

impl Compute for ConfigCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> oxide_fw_graph::Result<NodeOutput> {
        let mode = ctx
            .env
            .get("OXIDE_EXAMPLE_MODE")
            .unwrap_or_else(|| "unset".to_string());
        println!("  [config] computed for mode `{mode}`");
        Ok(NodeOutput {
            hash: blake3::hash(mode.as_bytes()),
            artifact: None,
            deps: std::collections::BTreeSet::new(),
        })
    }
}

/// The bundle node: demands the config node — an `Input::Node` edge — and
/// mixes the config's hash into its own.
struct BundleCompute {
    config_key: NodeKey,
}

impl Compute for BundleCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> oxide_fw_graph::Result<NodeOutput> {
        ctx.check_cancel()?;
        let config = ctx.dep(&self.config_key)?;
        let mixed = format!("bundle({})", config.hash);
        println!("  [bundle] computed over config {}", config.hash);
        Ok(NodeOutput {
            hash: blake3::hash(mixed.as_bytes()),
            artifact: None,
            deps: std::collections::BTreeSet::new(),
        })
    }
}

fn main() {
    let config_key = NodeKey::new("example://config");
    let bundle_key = NodeKey::new("example://bundle");

    let graph = DemandGraph::new();
    graph.set_env_source(env_source("dev"));
    let config = Arc::new(ConfigCompute);
    let bundle = Arc::new(BundleCompute {
        config_key: config_key.clone(),
    });
    graph.register(config_key.clone(), config.clone());
    graph.register(bundle_key.clone(), bundle.clone());

    println!("== 1. demand: get(bundle) computes config first, then bundle ==");
    let first = graph
        .get(&bundle_key, bundle.as_ref())
        .expect("first compute");
    println!(
        "  bundle hash {} | nodes {}",
        first.hash,
        graph.node_count()
    );

    println!("== 2. memoization: the same get answers without recomputing ==");
    let version_before = graph.version();
    let again = graph.get(&bundle_key, bundle.as_ref()).expect("memo hit");
    assert_eq!(first.hash, again.hash, "memo-fresh answer is identical");
    assert_eq!(
        version_before,
        graph.version(),
        "no recompute, no version bump"
    );
    println!("  version stayed {version_before}, hash unchanged");

    println!("== 3. invalidation: the env change follows the dep edges ==");
    graph.set_env_source(env_source("prod"));
    let mode = "prod";
    let invalidated = graph.invalidate(&[Input::Env {
        key: "OXIDE_EXAMPLE_MODE".to_string(),
        value_hash: blake3::hash(mode.as_bytes()),
    }]);
    println!("  invalidated: {invalidated:?}");
    assert_eq!(
        invalidated.len(),
        2,
        "the observer AND its downstream node both drop"
    );
    let recomputed = graph.get(&bundle_key, bundle.as_ref()).expect("recompute");
    assert_ne!(first.hash, recomputed.hash, "the fresh computation moves");
    println!("  bundle hash {} (was {})", recomputed.hash, first.hash);
}
