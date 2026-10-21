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

use oxide_graph::{Compute, ComputeCtx, DemandGraph, NodeKey, NodeOutput, Result as GraphResult};
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

/// One route's emitted client chunks and each chunk's blake3. This is the
/// diff target a per-chunk incremental build would use. Rolldown 1.2.13
/// exposes no per-module incremental API, so the build cannot yet act on a
/// per-chunk change — but recording the hashes makes "did this route's
/// chunks actually change?" measurable rather than a guess.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteChunkRecord {
    pub route_id: String,
    /// `(file name, blake3 hex)` for each chunk this route's entry reaches.
    pub chunks: Vec<(String, String)>,
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
    /// Each route's emitted chunks and their hashes.
    #[serde(default)]
    pub route_chunks: Vec<RouteChunkRecord>,
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

/// Reads a file and returns its blake3. On top of the `Input::File` the
/// recording `FileTracker` adds, this node **also** records one
/// `Input::Resolver` edge per bare specifier the file imports, and one
/// `Input::PluginOptions` edge per configured `kind:js` plugin that this
/// file path is.
///
/// Both variants existed in the graph enum but nothing constructed them —
/// §5-B. With these recordings they are real invalidation edges: a lockfile
/// bump that changes a resolved package invalidates every node whose
/// imports it affects, and a change to a plugin's options invalidates the
/// module the plugin transforms.
struct FileCompute {
    path: PathBuf,
    /// `(plugin_name, options_hash, plugin_path)` for every configured
    /// `kind:js` plugin. Only the entries whose `plugin_path` equals
    /// `self.path` are recorded (a source file is not a plugin entry unless
    /// the config says so).
    plugin_options: Vec<(String, blake3::Hash, PathBuf)>,
}

impl Compute for FileCompute {
    fn compute(&self, ctx: &mut ComputeCtx) -> GraphResult<NodeOutput> {
        let bytes = ctx.fs.read_file(&self.path)?;

        // §5-B: a Resolver edge per bare specifier this file imports. The
        // "resolved hash" for v0.1 is the specifier's own hash — the richer
        // identity (path + package version + matched conditions) lives in
        // oxide-bundler's resolver, which is not a Compute. Recording the
        // specifier is what makes `invalidate(&[Input::Resolver{...}])`
        // possible; a later build can bump the resolved hash when the
        // resolver observes a lockfile change.
        if let Ok(text) = std::str::from_utf8(&bytes) {
            for spec in scan_bare_imports(text) {
                let resolved_hash = blake3::hash(spec.as_bytes());
                ctx.record_resolver(&spec, &self.path.to_string_lossy(), resolved_hash);
            }
        }

        // §5-B: a PluginOptions edge when this file *is* a configured
        // plugin entry. The plugin's own source change is already captured
        // by `Input::File`; this edge ties it to the *options* the config
        // declares, so an options change invalidates the module the plugin
        // transforms.
        for (name, opts_hash, plugin_path) in &self.plugin_options {
            if plugin_path == &self.path {
                ctx.record_plugin_options(name, *opts_hash);
            }
        }

        Ok(NodeOutput {
            hash: blake3::hash(&bytes),
            artifact: None,
            deps: Default::default(),
        })
    }
}

/// Extract every bare (non-relative) module specifier from a JS/TS source.
/// Matches `from '…'`, `from "…"`, `import('…')`, and `require('…')`.
/// Relative specifiers (`.` / `..`) and absolute paths are skipped — those
/// resolve to a path, not to a package identity; the file's own content
/// hash covers them.
fn scan_bare_imports(src: &str) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for marker in ["from ", "import(", "require("] {
        let mut rest = src;
        while let Some(i) = rest.find(marker) {
            let after = &rest[i + marker.len()..];
            let after = after.trim_start();
            let Some(quote) = after.chars().next().filter(|c| *c == '\'' || *c == '"') else {
                // No quoted string after the marker — skip a byte and
                // continue so a malformed sequence does not spin.
                if after.is_empty() {
                    break;
                }
                rest = &after[1..];
                continue;
            };
            let body = &after[1..];
            let Some(end) = body.find(quote) else { break };
            let spec = body[..end].trim();
            let is_relative = spec.starts_with('.') || spec.starts_with('/');
            if !is_relative && !spec.is_empty() {
                out.insert(spec.to_string());
            }
            rest = &body[end + 1..];
        }
    }
    out.into_iter().collect()
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

/// A FileCompute with no plugin-options edges — used for chunk / manifest /
/// typegen nodes, which are not plugin entries.
fn file_compute(path: &Path) -> Arc<dyn Compute> {
    Arc::new(FileCompute {
        path: path.to_path_buf(),
        plugin_options: Vec::new(),
    })
}

/// A FileCompute that records §5-B plugin-options edges — used for module
/// source files, which a `kind:js` plugin may transform. `plugin_options`
/// carries one entry per configured plugin.
fn file_compute_with_plugins(
    path: &Path,
    plugin_options: &[(String, blake3::Hash, PathBuf)],
) -> Arc<dyn Compute> {
    Arc::new(FileCompute {
        path: path.to_path_buf(),
        plugin_options: plugin_options.to_vec(),
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
    /// §5-B: `(plugin_name, options_hash, plugin_path)` for every
    /// configured `kind:js` plugin. `cmd_build` computes these from the
    /// config; `file_compute_with_plugins` records the edges for module
    /// files whose path matches a plugin entry.
    pub plugin_options: Vec<(String, blake3::Hash, PathBuf)>,
    /// Each route's emitted chunks and their hashes.
    pub route_chunks: Vec<RouteChunkRecord>,
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
        // §5-B: the plugin-aware constructor records a PluginOptions edge
        // for every configured plugin that this file path happens to be.
        let comp = file_compute_with_plugins(src, &inputs.plugin_options);
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
        route_chunks: inputs.route_chunks,
    };
    let path = snapshot_path(inputs.rootp);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&snap)?)?;
    Ok(snap)
}
