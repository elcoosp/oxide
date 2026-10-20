//! §5-A conservative landing: a real `DemandGraph` over a build's inputs.
//!
//! The pipeline itself (`cmd_build`) still runs to completion — this module
//! **records** the graph after a build, so:
//!
//!   * `oxide analyze` can report how many nodes were recomputed in the
//!     last build;
//!   * the MCP `oxide_graph` tool can attach a `graph` section to its
//!     manifest summary;
//!   * a future incremental build has a node table to diff against.
//!
//! A "node" is one of:
//!   * `module:<abs-path>` — one per route source file the build consumed.
//!   * `chunk:<env>/<path>` — one per emitted `.js` file (client + SSR).
//!   * `manifest:<env>` — one per manifest JSON. Its hash transitively
//!     covers every chunk node (recorded via `ctx.dep`).
//!   * `typegen:<name>` — one per generated `*.d.ts` / `*.json` typegen file.
//!
//! The snapshot lives at `.oxide/graph.json` and is what `analyze` and the
//! MCP tool read. Driving the *bundler* node-by-node — so that touching one
//! component re-bundles exactly one chunk — is the follow-up that this
//! snapshot makes measurable.

use oxide_graph::{
    Compute, ComputeCtx, DemandGraph, NodeKey, NodeOutput, Result as GraphResult,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SNAPSHOT_VERSION: &str = "v1";
const SNAPSHOT_REL: &str = ".oxide/graph.json";

pub fn module_key(path: &Path) -> NodeKey {
    NodeKey::new(format!("module:{}", path.display()))
}
pub fn chunk_key(env: &str, path: &Path) -> NodeKey {
    NodeKey::new(format!("chunk:{env}/{}", path.display()))
}
pub fn manifest_key(env: &str) -> NodeKey {
    NodeKey::new(format!("manifest:{env}"))
}
pub fn typegen_key(name: &str) -> NodeKey {
    NodeKey::new(format!("typegen:{name}"))
}

fn kind_of(key: &str) -> &'static str {
    if key.starts_with("module:") {
        "module"
    } else if key.starts_with("chunk:") {
        "chunk"
    } else if key.starts_with("manifest:") {
        "manifest"
    } else if key.starts_with("typegen:") {
        "typegen"
    } else {
        "unknown"
    }
}

/// One row of the persisted node table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeRow {
    pub key: String,
    pub kind: String,
    pub hash: String,
    pub dep_count: usize,
}

/// The full snapshot persisted to `.oxide/graph.json`.
/// One prerender record: a route + the on-disk output it produced + the
/// source hash that produced it. A matching hash + existing file lets the
/// next build skip the render entirely (§5-A full incremental).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutePrerenderRecord {
    pub route_id: String,
    pub url: String,
    pub status: u16,
    /// Path relative to the project root (`dist/prerendered/...`).
    pub rel_out: String,
    /// blake3 of every source that fed this route's render: the serialized
    /// `OxideConfig` (a config change flushes every route — conservative
    /// but correct), the page file, its `page.server.ts`, every layout in
    /// the chain (both `layout.*` and `layout.server.*`), and the error
    /// boundary when one wraps the route.
    pub source_hash: String,
    /// The build-nonce the saved HTML was rendered with. On reuse, replace
    /// this with the current build's nonce so the file's `nonce="…"`
    /// attributes stay in sync with the CSP header the server emits. A
    /// reuse that skips this step would serve HTML whose inline scripts
    /// the browser blocks.
    pub nonce: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GraphSnapshot {
    pub version: String,
    pub build_id: String,
    pub generated_at_unix_ms: u64,
    pub node_count: usize,
    pub recomputed_count: usize,
    pub previous_node_count: usize,
    pub nodes: Vec<NodeRow>,
    /// §5-A full incremental: one record per prerendered route. Empty for a
    /// build that prerendered nothing; absent (older snapshot) deserializes
    /// as empty.
    #[serde(default)]
    pub route_prerenders: Vec<RoutePrerenderRecord>,
}

pub fn snapshot_path(rootp: &Path) -> PathBuf {
    rootp.join(SNAPSHOT_REL)
}

/// Read the persisted snapshot, or an empty one when missing/corrupt.
pub fn load_snapshot(rootp: &Path) -> GraphSnapshot {
    std::fs::read(snapshot_path(rootp))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

// ─── Compute impls ─────────────────────────────────────────────────────────

/// Reads a file and returns its blake3. The recorded `Input::File` is what
/// the graph's invalidation walks in a future incremental build.
struct FileCompute {
    path: PathBuf,
}

impl Compute for FileCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> GraphResult<NodeOutput> {
        let bytes = ctx.fs.read_file(&self.path)?;
        Ok(NodeOutput {
            hash: blake3::hash(&bytes),
            artifact: None,
            deps: Default::default(),
        })
    }
}

/// A manifest node: its hash covers the manifest bytes plus the hash of
/// every chunk node it names. This gives the graph its transitive
/// manifest → chunk edges.
struct ManifestCompute {
    path: PathBuf,
    chunk_keys: Vec<NodeKey>,
}

impl Compute for ManifestCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> GraphResult<NodeOutput> {
        let mut h = blake3::Hasher::new();
        for k in &self.chunk_keys {
            if let Ok(o) = ctx.dep(k) {
                h.update(o.hash.as_bytes());
            }
        }
        let bytes = ctx.fs.read_file(&self.path)?;
        h.update(&bytes);
        Ok(NodeOutput {
            hash: h.finalize(),
            artifact: None,
            deps: Default::default(),
        })
    }
}

fn file_compute(path: &Path) -> Arc<dyn Compute> {
    Arc::new(FileCompute {
        path: path.to_path_buf(),
    })
}

// ─── public entry point ────────────────────────────────────────────────────

pub struct RecordInputs<'a> {
    pub rootp: &'a Path,
    pub entries: &'a BTreeMap<String, PathBuf>,
    pub client_chunks: &'a [PathBuf],
    pub server_chunks: &'a [PathBuf],
    pub client_manifest_path: &'a Path,
    pub server_manifest_path: &'a Path,
    pub build_id: &'a str,
    /// §5-A full incremental: this build's per-route prerender records.
    /// The next build's reuse check reads these back from the snapshot.
    pub route_prerenders: Vec<RoutePrerenderRecord>,
}

/// Record a graph over `inputs`, compare against the previous snapshot, and
/// persist the new one. Never fails the build — the caller logs on `Err`.
pub fn record_and_persist(inputs: RecordInputs<'_>) -> std::io::Result<GraphSnapshot> {
    let previous = load_snapshot(inputs.rootp);
    let prev_hash: BTreeMap<String, String> = previous
        .nodes
        .iter()
        .map(|r| (r.key.clone(), r.hash.clone()))
        .collect();

    let graph = DemandGraph::new();
    let mut all_keys: Vec<String> = Vec::new();

    // 1. Route modules: one node per source file the build consumed.
    for src in inputs.entries.values() {
        let mk = module_key(src);
        let comp = file_compute(src);
        graph.register(mk.clone(), Arc::clone(&comp));
        let _ = graph.get(mk.clone(), comp.as_ref());
        all_keys.push(mk.0.to_string());
    }
    // Also record the boot entry when one was synthesised.
    let boot_src = inputs.rootp.join(".oxide/island-boot.mjs");
    if boot_src.is_file() {
        let mk = module_key(&boot_src);
        let comp = file_compute(&boot_src);
        graph.register(mk.clone(), Arc::clone(&comp));
        let _ = graph.get(mk.clone(), comp.as_ref());
        all_keys.push(mk.0.to_string());
    }

    // 2. Chunks: one per emitted `.js` file.
    let mut client_chunk_keys: Vec<NodeKey> = Vec::new();
    for p in inputs.client_chunks {
        let ck = chunk_key("client", p);
        let comp = file_compute(p);
        graph.register(ck.clone(), Arc::clone(&comp));
        let _ = graph.get(ck.clone(), comp.as_ref());
        all_keys.push(ck.0.to_string());
        client_chunk_keys.push(ck);
    }
    let mut server_chunk_keys: Vec<NodeKey> = Vec::new();
    for p in inputs.server_chunks {
        let ck = chunk_key("ssr", p);
        let comp = file_compute(p);
        graph.register(ck.clone(), Arc::clone(&comp));
        let _ = graph.get(ck.clone(), comp.as_ref());
        all_keys.push(ck.0.to_string());
        server_chunk_keys.push(ck);
    }

    // 3. Manifests: depend on every chunk in their env.
    for (env, mp, cks) in [
        ("client", inputs.client_manifest_path, &client_chunk_keys),
        ("server", inputs.server_manifest_path, &server_chunk_keys),
    ] {
        let mk = manifest_key(env);
        let comp: Arc<dyn Compute> = Arc::new(ManifestCompute {
            path: mp.to_path_buf(),
            chunk_keys: cks.clone(),
        });
        graph.register(mk.clone(), Arc::clone(&comp));
        let _ = graph.get(mk.clone(), comp.as_ref());
        all_keys.push(mk.0.to_string());
    }

    // 4. Typegen files (routes.d.ts, routes.json, messages.d.ts).
    for (name, rel) in [
        ("routes", ".oxide/types/routes.d.ts"),
        ("routes_json", ".oxide/types/routes.json"),
        ("messages", ".oxide/types/messages.d.ts"),
    ] {
        let p = inputs.rootp.join(rel);
        if p.is_file() {
            let tk = typegen_key(name);
            let comp = file_compute(&p);
            graph.register(tk.clone(), Arc::clone(&comp));
            let _ = graph.get(tk.clone(), comp.as_ref());
            all_keys.push(tk.0.to_string());
        }
    }

    // 5. Snapshot: dedupe keys, read every node, compare to previous.
    all_keys.sort();
    all_keys.dedup();

    let mut nodes: Vec<NodeRow> = Vec::with_capacity(all_keys.len());
    let mut recomputed = 0usize;
    for key_str in &all_keys {
        let k = NodeKey::new(key_str.as_str());
        let Some(o) = graph.peek(&k) else { continue };
        let hash_str = o.hash.to_hex().to_string();
        if prev_hash
            .get(key_str)
            .map(|h| h != &hash_str)
            .unwrap_or(true)
        {
            recomputed += 1;
        }
        nodes.push(NodeRow {
            key: key_str.clone(),
            kind: kind_of(key_str).to_string(),
            hash: hash_str,
            dep_count: o.deps.len(),
        });
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let snap = GraphSnapshot {
        version: SNAPSHOT_VERSION.to_string(),
        build_id: inputs.build_id.to_string(),
        generated_at_unix_ms: now_ms,
        node_count: nodes.len(),
        recomputed_count: recomputed,
        previous_node_count: previous.node_count,
        nodes,
        route_prerenders: inputs.route_prerenders,
    };
    let path = snapshot_path(inputs.rootp);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&snap)?)?;
    Ok(snap)
}
