//! oxide-bundler — bundler integration + environments + manifests (spec §6.4, D3).
//!
//! Rolldown is used as a library behind the `Bundler` trait (D3). This workspace ships
//! a functional `ChunkBundler` (ESM resolution, route-based code splitting, shared-chunk
//! heuristics, content-hashed output, client/server manifests, tree-shake guard) so the
//! pipeline is end-to-end testable; docs/deviations.md records the Rolldown swap point.
//!
//! **Substitution note (D3):** `ChunkBundler` is a functional stand-in, not Rolldown.
//! It resolves (§6.2 step 1), scans imports via `oxide-compiler`, splits per route,
//! hashes chunks by content, writes files + manifests, and tolerates cycles — enough
//! for the pipeline to be end-to-end real. Cross-chunk *binding* wiring is naive:
//! co-located modules rely on ESM top-level declarations, imports pointing into other
//! chunks are rewritten to the chunk file (shared chunks re-export scanned names), and
//! `export ... from` re-exports are not rewritten. Rolldown replaces all of it behind
//! the [`Bundler`] trait.

#![forbid(unsafe_code)]

mod minify;
mod resolve;
mod rolldown_backend;
mod prebundle;

use blake3::Hasher;
use oxide_compiler::{is_server_module_path, scan_import_spans};
use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

pub use rolldown_backend::RolldownBundler;
pub use prebundle::{prebundle, PrebundleManifest, PrebundleOptions, PrebundledPackage};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

fn io_err(what: &str, e: std::io::Error) -> OxideError {
    err("BLD-IO", format!("{what}: {e}"))
}

fn json_err(e: serde_json::Error) -> OxideError {
    err("BLD-JSON", format!("manifest serialization failed: {e}"))
}

/// Bundle environments (§6.1): `client`, `ssr`, optional `edge`. Each has its own
/// resolve conditions, platform, target, externals, define set and plugin list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BundleEnv {
    #[default]
    Client,
    Ssr,
    Edge,
}

impl BundleEnv {
    pub fn as_str(&self) -> &'static str {
        match self {
            BundleEnv::Client => "client",
            BundleEnv::Ssr => "ssr",
            BundleEnv::Edge => "edge",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BundleOptions {
    pub env: BundleEnv,
    /// Entry points: route id → source path.
    pub entries: BTreeMap<String, PathBuf>,
    pub out_dir: PathBuf,
    pub minify: bool,
    pub sourcemap: bool,
    /// Externals (never bundled).
    pub externals: Vec<String>,
    /// `define` replacements.
    pub define: BTreeMap<String, String>,
    /// tsconfig `paths` mapping for the resolver.
    pub tsconfig_paths: BTreeMap<String, Vec<String>>,
}

/// A resolved module in the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub specifier: String,
    pub resolved: PathBuf,
    pub source: String,
    pub hash: String,
    pub imports: Vec<String>,
    /// CSS or other side-effect asset.
    pub is_asset: bool,
    /// true if this module is a `*.server.ts` (never in client output, §6.4 guard).
    pub server_only: bool,
}

/// Output chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub name: String,
    pub file: String,
    pub hash: String,
    pub modules: Vec<PathBuf>,
    pub imports: Vec<String>,
    pub css: Vec<String>,
    pub kind: ChunkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkKind {
    Entry,
    Shared,
    Asset,
}

/// Client manifest (§6.4): route → entry chunks, CSS, preload order, assets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientManifest {
    pub build_id: String,
    pub routes: BTreeMap<String, RouteBundle>,
    pub assets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteBundle {
    pub entry_chunk: String,
    /// Preload order (modulepreload links, critical first, §10.2 step 4).
    pub preload: Vec<String>,
    pub css: Vec<String>,
    /// Estimated JS bytes on the wire (minified) for budget checks (§21).
    pub js_bytes: u64,
}

/// Server manifest (§6.4): route → server module, adapter entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerManifest {
    pub build_id: String,
    pub routes: BTreeMap<String, ServerRoute>,
    pub adapter_entry: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerRoute {
    pub module: String,
    pub render_mode: Option<String>,
    pub tags: Vec<String>,
    /// Prerendered routes list their output files here (§10.5).
    pub prerendered: Option<String>,
}

/// The bundler trait (D3 isolation point; Rolldown slots in here).
pub trait Bundler: Send + Sync {
    fn bundle(&self, options: &BundleOptions) -> Result<BundleOutput>;
}

#[derive(Debug, Clone, Default)]
pub struct BundleOutput {
    pub modules: Vec<Module>,
    pub chunks: Vec<Chunk>,
    pub client_manifest: Option<ClientManifest>,
    pub server_manifest: Option<ServerManifest>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Functional bundler: resolve (relative + tsconfig paths + node_modules `exports`),
/// scan imports via `oxide-compiler`, topo-sort, split per route with a shared-chunk
/// heuristic (modules used by ≥2 entries), emit content-hashed chunks + manifests.
pub struct ChunkBundler {
    pub root: PathBuf,
}

impl Bundler for ChunkBundler {
    fn bundle(&self, options: &BundleOptions) -> Result<BundleOutput> {
        self.bundle_impl(options)
    }
}

impl ChunkBundler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a specifier (§6.2 step 1): relative, tsconfig `paths`, package `exports`.
    pub fn resolve(
        &self,
        specifier: &str,
        from: &Path,
        env: BundleEnv,
        paths: &BTreeMap<String, Vec<String>>,
    ) -> Result<PathBuf> {
        resolve::resolve(&self.root, specifier, from, env, paths)
    }

    /// Shared-chunk decision: module used by ≥ `threshold` entries → shared chunk.
    pub fn shared_threshold(&self) -> usize {
        2
    }

    /// Content-hash a chunk: blake3 of sorted module hashes + chunk kind + name.
    pub fn content_hash(parts: &[&[u8]]) -> String {
        let mut h = Hasher::new();
        for p in parts {
            h.update(&(p.len() as u32).to_le_bytes());
            h.update(p);
        }
        h.finalize().to_hex().to_string()[..12].to_string()
    }

    fn abs(&self, p: &Path) -> PathBuf {
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        resolve::normalize(&p)
    }

    fn bundle_impl(&self, options: &BundleOptions) -> Result<BundleOutput> {
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut modules: Vec<Module> = Vec::new();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        // resolved module path → resolved dependency paths (env-filtered)
        let mut edges: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut entry_of: BTreeMap<String, PathBuf> = BTreeMap::new();

        // ------------------------------------------------------ 1. walk the graph
        let mut queue: VecDeque<(String, PathBuf)> = VecDeque::new();
        for (route, path) in &options.entries {
            let rp = self.abs(path);
            if !rp.is_file() {
                return Err(err(
                    "BLD-RESOLVE",
                    format!(
                        "entry module `{}` for route `{route}` not found",
                        rp.display()
                    ),
                ));
            }
            entry_of.insert(route.clone(), rp.clone());
            queue.push_back((path.to_string_lossy().to_string(), rp));
        }
        while let Some((spec, rp)) = queue.pop_front() {
            if !seen.insert(rp.clone()) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&rp) else {
                diagnostics.push(Diagnostic::error(
                    "BLD-RESOLVE",
                    format!("cannot read module `{}`", rp.display()),
                ));
                continue;
            };
            let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
            let is_css = rp.extension().is_some_and(|e| e == "css");
            let is_asset = spec.contains('?') || is_css;
            let server_only = is_server_module_path(&rp.to_string_lossy());
            // §6.4: server-only modules are stubbed out of the client graph — their
            // imports are dropped silently (the stub transform replaces usage).
            if options.env == BundleEnv::Client && server_only {
                continue;
            }
            let mut imports: Vec<String> = Vec::new();
            let mut deps: Vec<PathBuf> = Vec::new();
            if !is_asset {
                for sp in scan_import_spans(&source) {
                    if !imports.contains(&sp.specifier) {
                        imports.push(sp.specifier.clone());
                    }
                    if options.externals.iter().any(|x| x == &sp.specifier) {
                        continue;
                    }
                    match self.resolve(&sp.specifier, &rp, options.env, &options.tsconfig_paths) {
                        Ok(dep) => {
                            if options.env == BundleEnv::Client
                                && is_server_module_path(&dep.to_string_lossy())
                            {
                                continue;
                            }
                            if !deps.contains(&dep) {
                                deps.push(dep.clone());
                            }
                            queue.push_back((sp.specifier.clone(), dep));
                        }
                        Err(e) => diagnostics.push(e.0),
                    }
                }
            }
            edges.insert(rp.clone(), deps);
            modules.push(Module {
                specifier: spec,
                resolved: rp,
                source,
                hash,
                imports,
                is_asset,
                server_only,
            });
        }
        if options.env == BundleEnv::Client && modules.iter().any(|m| m.server_only) {
            // unreachable in practice; kept as a belt-and-braces §6.4 invariant
            return Err(err(
                "BLD-GUARD",
                "server-only module leaked into client graph",
            ));
        }

        // ------------------------------------------------ 2. reachability + split
        let mut reach: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
        for (route, erp) in &entry_of {
            let mut set = BTreeSet::new();
            let mut stack = vec![erp.clone()];
            while let Some(cur) = stack.pop() {
                if !set.insert(cur.clone()) {
                    continue;
                }
                if let Some(deps) = edges.get(&cur) {
                    stack.extend(deps.iter().cloned());
                }
            }
            reach.insert(route.clone(), set);
        }
        let mut owner_count: BTreeMap<PathBuf, usize> = BTreeMap::new();
        let mut owners: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        for (route, set) in &reach {
            for m in set {
                *owner_count.entry(m.clone()).or_insert(0) += 1;
                owners.entry(m.clone()).or_default().push(route.clone());
            }
        }

        let is_css = |m: &Module| m.resolved.extension().is_some_and(|e| e == "css");
        let threshold = self.shared_threshold();
        // Assets (css, `?query`) never form their own JS chunk; their chunk
        // assignment is derived from their importer below.
        let mut shared: Vec<PathBuf> = modules
            .iter()
            .filter(|m| {
                !m.is_asset && owner_count.get(&m.resolved).copied().unwrap_or(0) >= threshold
            })
            .map(|m| m.resolved.clone())
            .collect();
        shared.sort();

        let mut chunk_of: BTreeMap<PathBuf, String> = BTreeMap::new();
        for (i, p) in shared.iter().enumerate() {
            chunk_of.insert(p.clone(), format!("shared-{i}"));
        }
        let mut entry_chunks: BTreeMap<String, String> = BTreeMap::new();
        for (route, erp) in &entry_of {
            let name = route.replace(['/', '\\'], "-");
            chunk_of.insert(erp.clone(), name.clone());
            entry_chunks.insert(route.clone(), name);
        }
        for m in &modules {
            if chunk_of.contains_key(&m.resolved) || m.is_asset {
                continue; // assets are attached via their importer (§6.7 css handling)
            }
            let cnt = owner_count.get(&m.resolved).copied().unwrap_or(0);
            if cnt == 1 {
                let owner = &owners[&m.resolved][0];
                chunk_of.insert(m.resolved.clone(), entry_chunks[owner].clone());
            } else if cnt == 0 {
                if let Some((_, name)) = entry_chunks.iter().next() {
                    chunk_of.insert(m.resolved.clone(), name.clone());
                }
            }
        }

        // CSS modules ride the chunk of their (first, deterministic) importer
        let mut importers_of: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for (from, deps) in &edges {
            for d in deps {
                importers_of
                    .entry(d.clone())
                    .or_default()
                    .push(from.clone());
            }
        }
        let mut css_chunk_of: BTreeMap<PathBuf, String> = BTreeMap::new();
        for m in modules.iter().filter(|m| m.is_asset && is_css(m)) {
            if let Some(owner) = importers_of
                .get(&m.resolved)
                .and_then(|ims| ims.iter().find_map(|i| chunk_of.get(i).cloned()))
            {
                css_chunk_of.insert(m.resolved.clone(), owner);
            }
        }

        // -------------------------------------------------- 3. import cycle check
        let seeds: Vec<PathBuf> = entry_of.values().cloned().collect();
        let (topo, cycles) = topo_order(&seeds, &edges);
        for chain in &cycles {
            diagnostics.push(
                Diagnostic::error("BLD-CYCLE", format!("import cycle detected: {chain}"))
                    .with_fix("break the cycle by extracting the shared code into its own module"),
            );
        }
        // Deterministic order: topo when acyclic, lexicographic fallback on cycles.
        let mut order: Vec<PathBuf> = match topo {
            Some(o) => o,
            None => modules.iter().map(|m| m.resolved.clone()).collect(),
        };
        order.sort(); // group members are filtered from this global order below

        // ------------------------------------------------- 4. build + name chunks
        let mut chunk_names: Vec<String> = chunk_of.values().cloned().collect();
        chunk_names.sort();
        chunk_names.dedup();
        let by_path: BTreeMap<PathBuf, &Module> =
            modules.iter().map(|m| (m.resolved.clone(), m)).collect();

        struct Built {
            name: String,
            kind: ChunkKind,
            js_mods: Vec<PathBuf>,
            imports: Vec<String>,
            hash: String,
            file: String,
            css_files: Vec<(PathBuf, String)>, // (source path, output file)
        }
        let mut built: Vec<Built> = Vec::new();
        for name in &chunk_names {
            let members: Vec<&Module> = modules
                .iter()
                .filter(|m| {
                    chunk_of
                        .get(&m.resolved)
                        .map(|c| c == name)
                        .unwrap_or(false)
                })
                .collect();
            let js_mods: Vec<PathBuf> = members
                .iter()
                .filter(|m| !m.is_asset)
                .map(|m| m.resolved.clone())
                .collect();
            let css_mods: Vec<PathBuf> = css_chunk_of
                .iter()
                .filter(|(_, cn)| cn.as_str() == name)
                .map(|(p, _)| p.clone())
                .collect();
            let kind = if name.starts_with("shared-") {
                ChunkKind::Shared
            } else {
                ChunkKind::Entry
            };
            // chunk-level import graph (files of other chunks referenced by members)
            let mut imports: BTreeSet<String> = BTreeSet::new();
            for m in &js_mods {
                if let Some(deps) = edges.get(m) {
                    for d in deps {
                        if let Some(tc) = chunk_of.get(d) {
                            if tc != name {
                                imports.insert(tc.clone());
                            }
                        }
                    }
                }
            }
            // content hash over sorted module hashes + kind + name (deterministic)
            let mut hashes: Vec<&str> = members.iter().map(|m| m.hash.as_str()).collect();
            hashes.sort();
            let mut parts: Vec<&[u8]> = vec![
                name.as_bytes(),
                if kind == ChunkKind::Shared {
                    b"shared"
                } else {
                    b"entry"
                },
            ];
            for h in &hashes {
                parts.push(h.as_bytes());
            }
            let hash = Self::content_hash(&parts);
            let file = format!("{name}-{hash}.js");
            // css outputs
            let mut css_files: Vec<(PathBuf, String)> = Vec::new();
            for cm in &css_mods {
                let m = &by_path[cm];
                let stem = m
                    .resolved
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "style".into());
                let css_hash = Self::content_hash(&[m.hash.as_bytes()]);
                let css_file = format!("{stem}-{css_hash}.css");
                css_files.push((m.resolved.clone(), css_file));
            }
            built.push(Built {
                name: name.clone(),
                kind,
                js_mods,
                imports: imports.into_iter().collect(),
                hash,
                file,
                css_files,
            });
        }
        let file_of: BTreeMap<String, String> = built
            .iter()
            .map(|b| (b.name.clone(), b.file.clone()))
            .collect();
        let file_imports_of: BTreeMap<String, Vec<String>> = built
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    b.imports
                        .iter()
                        .map(|n| file_of[n].clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();

        // ----------------------------------------------------- 5. emit chunk files
        std::fs::create_dir_all(&options.out_dir).map_err(|e| io_err("mkdir failed", e))?;
        let mut bodies: BTreeMap<String, String> = BTreeMap::new();
        for b in &built {
            let mut body = String::new();
            for dep in &b.imports {
                if let Some(dep_file) = file_of.get(dep) {
                    body.push_str(&format!("import \"./{dep_file}\";\n"));
                }
            }
            for p in &order {
                if !b.js_mods.contains(p) {
                    continue;
                }
                let m = &by_path[p];
                let src =
                    rewrite_imports(&m.source, &b.name, m, options, &chunk_of, &file_of, self);
                body.push_str(&src);
                if !body.ends_with('\n') {
                    body.push('\n');
                }
            }
            if b.kind == ChunkKind::Shared {
                let mut names: BTreeSet<String> = BTreeSet::new();
                for p in &b.js_mods {
                    for n in export_names(&by_path[p].source) {
                        names.insert(n);
                    }
                }
                if !names.is_empty() {
                    let list: Vec<String> = names.into_iter().collect();
                    body.push_str(&format!("\nexport {{ {} }};\n", list.join(", ")));
                }
            }
            let mut body = if options.minify {
                minify::minify(&body)
            } else {
                body
            };
            if options.sourcemap {
                body.push_str(&format!("//# sourceMappingURL={}.map\n", b.file));
                std::fs::write(options.out_dir.join(format!("{}.map", b.file)), "{}\n")
                    .map_err(|e| io_err("sourcemap write failed", e))?;
            }
            std::fs::write(options.out_dir.join(&b.file), &body)
                .map_err(|e| io_err("chunk write failed", e))?;
            bodies.insert(b.file.clone(), body);
            // css files
            for (src_path, css_file) in &b.css_files {
                let src = &by_path[src_path].source;
                let css = if options.minify {
                    minify::minify(src)
                } else {
                    src.clone()
                };
                std::fs::write(options.out_dir.join(css_file), css)
                    .map_err(|e| io_err("css write failed", e))?;
            }
        }

        let chunks: Vec<Chunk> = built
            .iter()
            .map(|b| Chunk {
                name: b.name.clone(),
                file: b.file.clone(),
                hash: b.hash.clone(),
                modules: b.js_mods.clone(),
                // chunk-level import graph as emitted FILE names (entry → shared
                // chunk files it needs, §6.4)
                imports: file_imports_of[&b.name].clone(),
                css: b.css_files.iter().map(|(_, f)| f.clone()).collect(),
                kind: b.kind,
            })
            .collect();

        // chunk-level import graph by chunk name (for preload order / js_bytes)
        let mut imports_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in &built {
            imports_by_name.insert(b.name.clone(), b.imports.clone());
        }
        let js_size_of: BTreeMap<String, u64> = built
            .iter()
            .map(|b| {
                let size = bodies.get(&b.file).map(|s| s.len()).unwrap_or(0) as u64;
                (b.name.clone(), size)
            })
            .collect();

        // ------------------------------------------------------------ 6. manifests
        let mut sorted_hashes: Vec<&String> = chunks.iter().map(|c| &c.hash).collect();
        sorted_hashes.sort();
        sorted_hashes.dedup();
        let joined = sorted_hashes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let build_id = blake3::hash(joined.as_bytes()).to_hex().to_string()[..16].to_string();

        // Emit root-relative paths for the client manifest. Handles the macOS
        // `/var` → `/private/var` case where the resolver's output prefix
        // differs from the caller-supplied `root` string despite pointing at
        // the same file.
        let canon_root = self.root.canonicalize().ok();
        let rel_to_root = |p: &Path| -> String {
            if let Ok(r) = p.strip_prefix(&self.root) {
                return r.to_string_lossy().to_string();
            }
            if let Some(cr) = &canon_root {
                if let Ok(r) = p.strip_prefix(cr) {
                    return r.to_string_lossy().to_string();
                }
            }
            // Symlink-free fallback: try the canonical form of the file too.
            if let Ok(cp) = p.canonicalize() {
                if let Some(cr) = &canon_root {
                    if let Ok(r) = cp.strip_prefix(cr) {
                        return r.to_string_lossy().to_string();
                    }
                }
                if let Ok(r) = cp.strip_prefix(&self.root) {
                    return r.to_string_lossy().to_string();
                }
            }
            // Last resort: strip up to the trailing `src/` segment.
            let s = p.to_string_lossy();
            if let Some(idx) = s.find("/src/") {
                return s[idx + 1..].to_string();
            }
            s.to_string()
        };

        let css_of_name: BTreeMap<String, Vec<String>> = built
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    b.css_files.iter().map(|(_, f)| f.clone()).collect(),
                )
            })
            .collect();

        let mut client_manifest = None;
        let mut server_manifest = None;
        if options.env == BundleEnv::Client {
            let mut routes = BTreeMap::new();
            for (route, cname) in &entry_chunks {
                let mut seen_c: BTreeSet<String> = BTreeSet::new();
                let mut preload: Vec<String> = Vec::new();
                let mut css: Vec<String> = Vec::new();
                dfs_chunks(
                    cname,
                    &imports_by_name,
                    &file_of,
                    &css_of_name,
                    &mut seen_c,
                    &mut preload,
                    &mut css,
                );
                let js_bytes: u64 = seen_c.iter().map(|n| js_size_of[n]).sum();
                routes.insert(
                    route.clone(),
                    RouteBundle {
                        entry_chunk: file_of[cname].clone(),
                        preload,
                        css,
                        js_bytes,
                    },
                );
            }
            let mut assets = BTreeMap::new();
            for b in &built {
                for (src_path, css_file) in &b.css_files {
                    assets.insert(rel_to_root(src_path), css_file.clone());
                }
            }
            let manifest = ClientManifest {
                build_id: build_id.clone(),
                routes,
                assets,
            };
            std::fs::write(
                options.out_dir.join("client.manifest.json"),
                serde_json::to_string_pretty(&manifest).map_err(json_err)? + "\n",
            )
            .map_err(|e| io_err("manifest write failed", e))?;
            client_manifest = Some(manifest);
        } else {
            let mut routes = BTreeMap::new();
            for (route, cname) in &entry_chunks {
                routes.insert(
                    route.clone(),
                    ServerRoute {
                        module: file_of[cname].clone(),
                        // render_mode inference lives in cli/render (§12), left None here
                        render_mode: None,
                        tags: Vec::new(),
                        prerendered: None,
                    },
                );
            }
            let manifest = ServerManifest {
                build_id: build_id.clone(),
                routes,
                adapter_entry: "ssr/index.js".to_string(),
            };
            // tiny generated adapter entry importing all route modules
            let mut entry = String::from(
                "// Generated by oxide-bundler — adapter entry (§6.4). Do not edit.\n",
            );
            for (i, (_, cname)) in entry_chunks.iter().enumerate() {
                entry.push_str(&format!(
                    "import * as route_{i} from \"../{}\";\n",
                    file_of[cname]
                ));
            }
            entry.push_str("\nexport const routes = {\n");
            for (i, (route, _)) in entry_chunks.iter().enumerate() {
                entry.push_str(&format!("  \"{route}\": route_{i},\n"));
            }
            entry.push_str("};\n");
            let ssr_dir = options.out_dir.join("ssr");
            std::fs::create_dir_all(&ssr_dir).map_err(|e| io_err("mkdir failed", e))?;
            std::fs::write(ssr_dir.join("index.js"), entry)
                .map_err(|e| io_err("adapter entry write failed", e))?;
            std::fs::write(
                options.out_dir.join("server.manifest.json"),
                serde_json::to_string_pretty(&manifest).map_err(json_err)? + "\n",
            )
            .map_err(|e| io_err("manifest write failed", e))?;
            server_manifest = Some(manifest);
        }

        let mut out_modules = modules;
        out_modules.sort_by(|a, b| a.resolved.cmp(&b.resolved));

        Ok(BundleOutput {
            modules: out_modules,
            chunks,
            client_manifest,
            server_manifest,
            diagnostics,
        })
    }
}

/// Depth-first chunk walk from an entry chunk: preload file order (entry first)
/// plus the CSS of every visited chunk, deduplicated (§6.4, §10.2 step 4).
#[allow(clippy::too_many_arguments)]
fn dfs_chunks(
    cname: &str,
    imports_by_name: &BTreeMap<String, Vec<String>>,
    file_of: &BTreeMap<String, String>,
    css_of_name: &BTreeMap<String, Vec<String>>,
    seen: &mut BTreeSet<String>,
    preload: &mut Vec<String>,
    css: &mut Vec<String>,
) {
    if !seen.insert(cname.to_string()) {
        return;
    }
    preload.push(file_of[cname].clone());
    if let Some(files) = css_of_name.get(cname) {
        for f in files {
            if !css.contains(f) {
                css.push(f.clone());
            }
        }
    }
    if let Some(deps) = imports_by_name.get(cname) {
        for d in deps {
            dfs_chunks(d, imports_by_name, file_of, css_of_name, seen, preload, css);
        }
    }
}

/// Rewrite import statements of `m` for chunk assembly: imports resolved to the
/// same chunk are removed (co-located top-level declarations provide the bindings),
/// imports resolved to other chunks are rewritten to that chunk's file.
fn rewrite_imports(
    code: &str,
    own_chunk: &str,
    m: &Module,
    options: &BundleOptions,
    chunk_of: &BTreeMap<PathBuf, String>,
    file_of: &BTreeMap<String, String>,
    bundler: &ChunkBundler,
) -> String {
    let spans = scan_import_spans(code);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for sp in spans.iter().filter(|s| !s.dynamic) {
        let Ok(target) = bundler.resolve(
            &sp.specifier,
            &m.resolved,
            options.env,
            &options.tsconfig_paths,
        ) else {
            continue; // external / unresolvable — keep as-is
        };
        // §6.4 guard: server-only modules never ship to the client — drop the
        // import statement entirely (the compiler's stub pass rewrites usages).
        if options.env == BundleEnv::Client && is_server_module_path(&target.to_string_lossy()) {
            edits.push((sp.start, sp.end, String::new()));
            continue;
        }
        // CSS imports are extracted onto the chunk record (`Chunk::css`) — they
        // must not survive as runtime JS imports.
        if target.extension().is_some_and(|e| e == "css") {
            edits.push((sp.start, sp.end, String::new()));
            continue;
        }
        let Some(tc) = chunk_of.get(&target) else {
            continue;
        };
        if tc == own_chunk {
            edits.push((sp.start, sp.end, String::new()));
        } else if let Some(dep_file) = file_of.get(tc) {
            if let Some(off) = code[sp.start..sp.end].find(&sp.specifier) {
                let at = sp.start + off;
                edits.push((at, at + sp.specifier.len(), format!("./{dep_file}")));
            }
        }
    }
    if edits.is_empty() {
        return code.to_string();
    }
    let mut out = code.to_string();
    edits.sort_by_key(|&(s, _, _)| std::cmp::Reverse(s));
    for (s, e, text) in edits {
        out.replace_range(s..e, &text);
    }
    out
}

/// Exported binding names of a module (for shared-chunk re-export footers):
/// `export const|let|var|function|class NAME`, `export async function NAME`,
/// `export { a, b as c }`. `export default` is skipped (documented limitation).
fn export_names(code: &str) -> Vec<String> {
    use oxide_compiler::lexer::{lex, Tok};
    let toks = lex(code);
    let mut names: Vec<String> = Vec::new();
    let word = |t: &oxide_compiler::lexer::Token, w: &str| {
        t.kind == Tok::Ident && &code[t.start..t.end] == w
    };
    let mut i = 0usize;
    while i < toks.len() {
        if word(&toks[i], "export") {
            match toks.get(i + 1).map(|t| &t.kind) {
                Some(Tok::Ident) => {
                    let mut j = i + 1;
                    if word(&toks[j], "default") {
                        // `export default <expr|fn>` — no portable name (documented)
                        i = j + 1;
                        continue;
                    }
                    if word(&toks[j], "async") {
                        j += 1;
                    }
                    // skip declaration keywords: `export const|let|var|function|class NAME`
                    while matches!(toks.get(j).map(|t| &t.kind), Some(Tok::Ident))
                        && ["const", "let", "var", "function", "class"]
                            .iter()
                            .any(|k| word(&toks[j], k))
                    {
                        j += 1;
                    }
                    if let Some(t) = toks.get(j) {
                        if t.kind == Tok::Ident && !word(t, "default") {
                            names.push(code[t.start..t.end].to_string());
                        }
                    }
                    i = j + 1;
                    continue;
                }
                Some(Tok::Punct('{')) => {
                    let mut j = i + 2;
                    while j < toks.len() {
                        if toks[j].kind == Tok::Punct('}') {
                            break;
                        }
                        if toks[j].kind == Tok::Ident
                            && !word(&toks[j], "as")
                            && (j == i + 2 || toks[j - 1].kind == Tok::Punct(','))
                        {
                            names.push(code[toks[j].start..toks[j].end].to_string());
                        }
                        j += 1;
                    }
                    i = j + 1;
                    continue;
                }
                _ => {}
            }
        }
        i += 1;
    }
    names
}

/// Topological order over the module edges (visited-set DFS). Returns `None` order
/// when a cycle exists, plus one "a -> b -> a" chain message per unique back-edge.
fn topo_order(
    seeds: &[PathBuf],
    edges: &BTreeMap<PathBuf, Vec<PathBuf>>,
) -> (Option<Vec<PathBuf>>, Vec<String>) {
    #[derive(Clone, Copy, PartialEq)]
    enum C {
        White,
        Gray,
        Black,
    }
    let mut color: BTreeMap<PathBuf, C> = BTreeMap::new();
    let mut stack: Vec<PathBuf> = Vec::new();
    let mut order: Vec<PathBuf> = Vec::new();
    let mut cycles: BTreeSet<String> = BTreeSet::new();

    fn visit(
        node: &PathBuf,
        edges: &BTreeMap<PathBuf, Vec<PathBuf>>,
        color: &mut BTreeMap<PathBuf, C>,
        stack: &mut Vec<PathBuf>,
        order: &mut Vec<PathBuf>,
        cycles: &mut BTreeSet<String>,
    ) {
        color.insert(node.clone(), C::Gray);
        stack.push(node.clone());
        if let Some(deps) = edges.get(node) {
            for d in deps {
                match color.get(d).copied().unwrap_or(C::White) {
                    C::Gray => {
                        let start = stack.iter().position(|p| p == d).unwrap_or(0);
                        let mut chain: Vec<String> = stack[start..]
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect();
                        chain.push(d.display().to_string());
                        cycles.insert(chain.join(" -> "));
                    }
                    C::White => visit(d, edges, color, stack, order, cycles),
                    C::Black => {}
                }
            }
        }
        stack.pop();
        color.insert(node.clone(), C::Black);
        order.push(node.clone());
    }

    let mut all: BTreeSet<&PathBuf> = edges.keys().collect();
    for deps in edges.values() {
        all.extend(deps.iter());
    }
    let mut queue: VecDeque<PathBuf> = seeds.iter().cloned().collect();
    for k in &all {
        queue.push_back((*k).clone());
    }
    let mut seen = BTreeSet::new();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if color.get(&n).copied().unwrap_or(C::White) == C::White {
            visit(&n, edges, &mut color, &mut stack, &mut order, &mut cycles);
        }
    }
    if cycles.is_empty() {
        (Some(order), Vec::new())
    } else {
        (None, cycles.into_iter().collect())
    }
}
