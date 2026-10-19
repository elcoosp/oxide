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

pub mod svg_react;
mod minify;
mod resolve;
mod rolldown_backend;

use blake3::Hasher;
use oxide_compiler::{
    Env as CompilerEnv, OxcTransformer, TransformInput, Transformer as _, is_server_module_path,
    scan_import_spans,
};
use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

pub use rolldown_backend::RolldownBundler;

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
    /// Per-build secret for server-fn ids (§9.4). Rotates on every build so
    /// the unguessable URL prefix for a server function differs between
    /// deployments. Left empty by callers that don't care (tests); the
    /// bundler derives a deterministic fallback from the entry set when
    /// empty so tests remain reproducible.
    pub build_secret: String,
    /// §6.3 precompiled sources: canonical absolute path → plain JS/TS
    /// produced by the tooling pool (`@vue/compiler-sfc`, `svelte/compiler`,
    /// `babel-preset-solid`). The CLI populates this once per env before
    /// calling `bundle`. When a module's path appears here, the bundler
    /// uses the provided source and skips the disk read + the SFC refusal.
    pub precompiled: BTreeMap<PathBuf, String>,
    /// §6.3: the JSX runtime to import in the lowered output. `None` means
    /// the Oxc default (`react/jsx-runtime`). Solid 2.0 sets this to
    /// `@solidjs/web` — its `jsx-runtime` subpath is where `jsx`/`jsxs`
    /// live. The CLI reads the framework adapter's declared
    /// `jsxImportSource` and forwards it here.
    pub jsx_import_source: Option<String>,
    /// §15.1: an optional `transform` hook a plugin supplies. Called with
    /// `(module id, source)` before the compiler passes run, on every
    /// module inside the project root. Returns the rewritten source.
    ///
    /// `oxide-bundler` does not depend on `oxide-plugin` — that would pull
    /// `wasmtime` into the bundler for a feature most builds do not use.
    /// The CLI pre-loads a WASM component, wraps it, and hands the closure
    /// in; the bundler only knows "call this function".
    ///
    /// A hook that returns an error fails the module's transform; the
    /// diagnostic surfaces through the normal `BundleOutput::diagnostics`
    /// path.
    pub transform_hook: Option<TransformHook>,
    /// SPEC-15.2: an optional plugin `resolveId` hook. Consulted before the
    /// default resolver; a returned `Some(path)` overrides disk resolution.
    /// `None` (the common case) means no plugins are loaded.
    pub resolve_hook: Option<ResolveHook>,
    /// SPEC-15.2: an optional plugin `load` hook. Consulted before the
    /// disk read; a returned `Some(source)` overrides (virtual modules).
    pub load_hook: Option<LoadHook>,
    /// §6.6: an optional CSS transform. The bundler does not depend on
    /// `oxide-assets` (which pulls lightningcss + the image/font stack);
    /// the CLI builds a closure over `LightningCssBackend` and hands it in.
    /// `None` writes CSS verbatim (minified by the in-house `minify` when
    /// `minify: true`).
    pub css_transform: Option<CssTransformHook>,
    /// §4.2: when `false`, dynamic imports are inlined into the entry instead
    /// of split into separate chunks. A self-contained file is what a bare V8
    /// ESM host needs — a split CJS runtime lands in a chunk that exports the
    /// `require` wrapper, not the module's namespace.
    pub inline_dynamic_imports: bool,
    /// §4.2 / BUG-SOLID: explicit export conditions for the resolver, tried
    /// before the platform defaults. `None` means "use the platform's default
    /// conditions" (browser for Client/Edge, node for Ssr). The V8 config
    /// bundle sets `["worker","browser","import","module","default"]` so it
    /// matches the V8 loader (`v8_loader.rs`) — `worker`-first is what gives
    /// `@solidjs/web` its SSR build (the client build's `ssr()` returns
    /// undefined) while still giving `react-dom/server` a browser-compatible
    /// build with no `require('stream')`.
    pub resolve_conditions: Option<Vec<String>>,
    /// SPEC-16.6: virtual modules — the CLI generates a synthetic file (e.g.
    /// `.oxide/virtual/<hash>.svg.react.tsx` for a `.svg?react` import) and
    /// registers its content here. The bundler treats these paths as normal
    /// entries (reads from `precompiled` because the file does not exist on
    /// disk) and `resolve.rs` maps `.svg?react` specifiers to their virtual
    /// paths before the resolver runs.
    ///
    /// Key: the ORIGINAL specifier as written (`"./icon.svg?react"`). Value:
    /// `(absolute-virtual-path, generated-source)`. Two files importing the
    /// same relative spec are rare and, in that case, both resolve to the
    /// same virtual file — which is correct.
    pub virtual_modules: BTreeMap<String, (std::path::PathBuf, String)>,
}

/// §6.6: a CSS transform the CLI supplies. Called with
/// `(path, source, is_module, minify)` and returns the transformed CSS plus
/// any CSS-modules class-name map. Mirrors `TransformHook`; the newtype
/// exists because `BundleOptions` derives `Debug`.
#[derive(Clone)]
pub struct CssTransformHook(
    #[allow(clippy::type_complexity)]
    pub  std::sync::Arc<
        dyn Fn(&str, &str, bool, bool) -> Result<(String, BTreeMap<String, String>)> + Send + Sync,
    >,
);

impl CssTransformHook {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str, &str, bool, bool) -> Result<(String, BTreeMap<String, String>)>
            + Send
            + Sync
            + 'static,
    {
        Self(std::sync::Arc::new(f))
    }

    pub fn call(
        &self,
        path: &str,
        source: &str,
        is_module: bool,
        minify: bool,
    ) -> Result<(String, BTreeMap<String, String>)> {
        (self.0)(path, source, is_module, minify)
    }
}

impl std::fmt::Debug for CssTransformHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CssTransformHook(..)")
    }
}

/// §15.1: a plugin-supplied transform, wrapped so it can live inside a
/// `#[derive(Debug, Clone, Default)]` struct. `BundleOptions` derives
/// `Debug` for diagnostics; a bare `Arc<dyn Fn…>` cannot, and the
/// newtype's `Debug` prints only that a hook is present (never its
/// internals, which are a loaded WASM component in the CLI's case).
#[derive(Clone)]
pub struct TransformHook(
    #[allow(clippy::type_complexity)]
    pub  std::sync::Arc<dyn Fn(&str, &str) -> Result<String> + Send + Sync>,
);

impl TransformHook {
    /// Wrap a closure so `BundleOptions` can hold it.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str, &str) -> Result<String> + Send + Sync + 'static,
    {
        Self(std::sync::Arc::new(f))
    }

    /// Run the hook.
    pub fn call(&self, id: &str, source: &str) -> Result<String> {
        (self.0)(id, source)
    }
}

/// SPEC-15.2: a plugin's `resolveId` hook. Returns `Some(path)` to OVERRIDE
/// the bundler's default resolution (the caller's path wins), or `None` to
/// let the resolver run. A plugin uses this to alias or virtualize modules.
///
/// Chaining semantics are in the CLI: the `JsPluginChain` calls every
/// plugin's `resolveId` in declaration order; the FIRST `Some(_)` wins
/// (Vite convention — a plugin that resolves wins over later plugins and
/// over the default).
pub struct ResolveHook(
    #[allow(clippy::type_complexity)]
    pub  std::sync::Arc<dyn Fn(&str, &str) -> Result<Option<PathBuf>> + Send + Sync>,
);

impl ResolveHook {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str, &str) -> Result<Option<PathBuf>> + Send + Sync + 'static,
    {
        Self(std::sync::Arc::new(f))
    }

    /// Run the hook. `id` is the specifier (as written in the source),
    /// `from` is the importing file's absolute path.
    pub fn call(&self, id: &str, from: &str) -> Result<Option<PathBuf>> {
        (self.0)(id, from)
    }
}

/// SPEC-15.2: a plugin's `load` hook. Returns `Some(source)` to override the
/// disk read (a virtual module), or `None` to let the bundler read the file.
/// The FIRST `Some(_)` wins across the chain; a plugin that returns `None`
/// passes to the next plugin and then to disk.
pub struct LoadHook(
    #[allow(clippy::type_complexity)]
    pub  std::sync::Arc<dyn Fn(&str) -> Result<Option<String>> + Send + Sync>,
);

impl LoadHook {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&str) -> Result<Option<String>> + Send + Sync + 'static,
    {
        Self(std::sync::Arc::new(f))
    }

    /// Run the hook. `id` is the resolved absolute path.
    pub fn call(&self, id: &str) -> Result<Option<String>> {
        (self.0)(id)
    }
}

impl Clone for ResolveHook {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for ResolveHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolveHook(..)")
    }
}

impl Clone for LoadHook {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for LoadHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoadHook(..)")
    }
}

impl std::fmt::Debug for TransformHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TransformHook(..)")
    }
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
    /// PERF-09: the source modules reachable from this route's entry — the
    /// data `oxide why` uses to attribute a module to a route.
    #[serde(default)]
    pub modules: Vec<PathBuf>,
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

/// SEC-08: scan the client bundle for `process.env.<NAME>` where NAME looks
/// like a secret (`SECRET|KEY|TOKEN|PASSWORD|PRIVATE`). A secret read in a
/// module that ships to the browser would leak it into the bundle — this
/// fails the build with `CMP-SECRET` naming the module and the env var.
/// `PUBLIC_*`/`NX_*`/`npm_*` are allow-listed (conventionally public).
pub fn scan_client_secrets(chunks: &[Chunk]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for chunk in chunks {
        let src = match std::fs::read_to_string(&chunk.file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // A conservative textual scan: `process.env.NAME`. This is the
        // build-time shape a bundler would otherwise inline.
        let mut rest = src.as_str();
        while let Some(i) = rest.find("process.env.") {
            let after = &rest[i + "process.env.".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if is_secret_like(&name) {
                out.push(Diagnostic::error(
                    "CMP-SECRET",
                    format!(
                        "`process.env.{name}` is read in client module `{}`;                          this would ship the secret to the browser. Move the                          read to a server module or rename to `PUBLIC_{name}`.",
                        chunk.file
                    ),
                ));
            }
            rest = &after[name.len().min(after.len())..];
            if rest.is_empty() {
                break;
            }
        }
    }
    out
}

fn is_secret_like(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("PUBLIC_")
        || upper.starts_with("NX_")
        || upper.starts_with("NPM_")
        || upper == "NODE_ENV"
    {
        return false;
    }
    ["SECRET", "KEY", "TOKEN", "PASSWORD", "PRIVATE"]
        .iter()
        .any(|m| upper.contains(m))
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

    /// Same as [`Self::resolve`] but with a `virtual_modules` table consulted
    /// before any disk resolution (SPEC-16.6). Callers that consume virtual
    /// modules (`.svg?react` and future `?w=` variants) use this entry point.
    pub fn resolve_with_virtual(
        &self,
        specifier: &str,
        from: &Path,
        env: BundleEnv,
        paths: &BTreeMap<String, Vec<String>>,
        virtual_modules: &BTreeMap<String, (std::path::PathBuf, String)>,
    ) -> Result<PathBuf> {
        resolve::resolve_with_virtual(&self.root, specifier, from, env, paths, virtual_modules)
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
        // §9.4 per-build secret. Empty from a test or an old caller → derive a
        // deterministic one from the entry set so successive runs in the same
        // process stay reproducible.
        let build_secret: String = if options.build_secret.is_empty() {
            let mut h = blake3::Hasher::new();
            for (route, path) in &options.entries {
                h.update(route.as_bytes());
                h.update(&[0u8]);
                h.update(path.to_string_lossy().as_bytes());
                h.update(&[0u8]);
            }
            h.finalize().to_hex().to_string()[..32].to_string()
        } else {
            options.build_secret.clone()
        };
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
            // §6.3: Vue/Svelte/Solid files must be lowered by their upstream
            // compilers (`@vue/compiler-sfc`, `svelte/compiler`,
            // `babel-preset-solid`) through the tooling pool (§4.7) before
            // the bundler can consume them. The CLI runs that pass and hands
            // the results in via `options.precompiled`. When a `.vue` /
            // `.svelte` file arrives here without a precompiled entry, the
            // caller skipped that pass — record a diagnostic naming the
            // reason rather than silently emitting the raw source.
            // SPEC-15.2: plugin `load` hook — consulted AFTER precompiled
            // (a Svelte SFC's precompiled JS is the source for that path),
            // but BEFORE disk. A `Some(src)` overrides the disk read entirely
            // (a virtual module).
            let load_hook_source = if options.precompiled.get(&rp).is_none() {
                if let Some(hook) = options.load_hook.as_ref() {
                    match hook.call(&rp.to_string_lossy()) {
                        Ok(v) => v,
                        Err(e) => {
                            diagnostics.push(Diagnostic::error(
                                "BLD-PLUGIN",
                                format!(
                                    "plugin load({:?}) failed: {} {}",
                                    rp.display(), e.0.code, e.0.message
                                ),
                            ));
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let source = if let Some(s) = options.precompiled.get(&rp) {
                s.clone()
            } else if let Some(s) = load_hook_source {
                s
            } else {
                match std::fs::read_to_string(&rp) {
                    Ok(s) => {
                        if let Some(fw) = unsupported_framework_ext(&rp) {
                            diagnostics.push(
                                Diagnostic::error(
                                    "BLD-FRAMEWORK",
                                    format!(
                                        "`{}` is a {fw} single-file component and was not \
                                         precompiled — the caller must run the tooling-pool \
                                         pass before invoking the bundler",
                                        rp.display()
                                    ),
                                )
                                .with_fix(
                                    "call oxide_bundler's precompile step or route the \
                                     build through `oxide build`, which does so automatically",
                                ),
                            );
                            continue;
                        }
                        s
                    }
                    Err(_) => {
                        diagnostics.push(Diagnostic::error(
                            "BLD-RESOLVE",
                            format!("cannot read module `{}`", rp.display()),
                        ));
                        continue;
                    }
                }
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
                    // SPEC-15.2: consult the plugin resolveId hook first.
                    // A `Some(path)` short-circuits the default resolver.
                    let rp_for_hook = rp.to_string_lossy().to_string();
                    let plugin_resolved = if let Some(hook) = options.resolve_hook.as_ref() {
                        match hook.call(&sp.specifier, &rp_for_hook) {
                            Ok(v) => v,
                            Err(e) => {
                                diagnostics.push(Diagnostic::error(
                                    "BLD-PLUGIN",
                                    format!(
                                        "plugin resolveId({:?}, {:?}) failed: {} {}",
                                        sp.specifier, rp_for_hook, e.0.code, e.0.message
                                    ),
                                ));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let resolved = match plugin_resolved {
                        Some(p) => Ok(p),
                        None => self.resolve_with_virtual(&sp.specifier, &rp, options.env, &options.tsconfig_paths, &options.virtual_modules),
                    };
                    match resolved {
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
                let rewritten =
                    rewrite_imports(&m.source, &b.name, m, options, &chunk_of, &file_of, self);
                let lowered = lower_module_to_js(
                    &rewritten,
                    &m.resolved,
                    options.env,
                    &build_secret,
                    options.jsx_import_source.as_deref(),
                    &mut diagnostics,
                );
                body.push_str(&lowered);
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
                let m = &by_path[src_path];
                let src = &m.source;
                let is_module = src_path
                    .file_name()
                    .map(|n| n.to_string_lossy().contains(".module."))
                    .unwrap_or(false);
                let css = if let Some(hook) = &options.css_transform {
                    let path = src_path.to_string_lossy();
                    match hook.call(&path, src, is_module, options.minify) {
                        Ok((code, _exports)) => code,
                        Err(e) => {
                            diagnostics.push(oxide_diagnostics::Diagnostic::error(
                                e.0.code,
                                format!("CSS transform failed for {path}: {}", e.0.message),
                            ));
                            src.clone()
                        }
                    }
                } else if options.minify {
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
                // PERF-09: the source modules in the reachable chunks.
                let modules: Vec<PathBuf> = chunk_of
                    .iter()
                    .filter(|(_, chunk)| seen_c.contains(chunk.as_str()))
                    .map(|(m, _)| m.clone())
                    .collect();
                routes.insert(
                    route.clone(),
                    RouteBundle {
                        entry_chunk: file_of[cname].clone(),
                        preload,
                        css,
                        js_bytes,
                        modules,
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
/// Returns the framework name if `path` is a single-file component that the
/// bundler cannot consume as-is (§6.3: Vue SFCs, Svelte components). The
/// tooling pool precompiles these; the bundler refuses them loudly so a
/// misconfigured build fails at the source, not at deploy time.
fn unsupported_framework_ext(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("vue") => Some("vue"),
        Some("svelte") => Some("svelte"),
        _ => None,
    }
}

fn lang_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") => "ts",
        Some("tsx") => "tsx",
        Some("jsx") => "jsx",
        Some("mjs") | Some("js") => "mjs",
        _ => "mjs",
    }
}

fn compiler_env(env: BundleEnv) -> CompilerEnv {
    match env {
        BundleEnv::Client => CompilerEnv::Client,
        BundleEnv::Ssr => CompilerEnv::Ssr,
        BundleEnv::Edge => CompilerEnv::Edge,
    }
}

/// Lower a rewritten TS/JSX module to plain JavaScript via the Oxc-backed
/// compiler transform. Diagnostics (parse errors, stub rewrites) are pushed
/// into the bundle's diagnostic vector; on failure the original source is
/// returned unchanged so the pipeline continues (and the diagnostic records
/// why the emitted chunk may be invalid).
fn lower_module_to_js(
    code: &str,
    resolved: &Path,
    env: BundleEnv,
    build_secret: &str,
    jsx_import_source: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let input = TransformInput {
        path: resolved.to_string_lossy().to_string(),
        code: code.to_string(),
        env: compiler_env(env),
        lang: lang_of(resolved),
        build_secret: build_secret.to_string(),
        dev: false,
        jsx_import_source: jsx_import_source.map(str::to_string),
    };
    match OxcTransformer.transform(&input) {
        Ok(out) => {
            diagnostics.extend(out.diagnostics);
            out.code
        }
        Err(e) => {
            diagnostics.push(e.0);
            code.to_string()
        }
    }
}

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
    use oxide_compiler::lexer::{Tok, lex};
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

#[cfg(test)]
mod sec08_tests {
    use super::*;
    #[test]
    fn scan_client_secrets_flags_secret_like_env() {
        assert!(is_secret_like("STRIPE_SECRET"));
        assert!(is_secret_like("API_KEY"));
        assert!(is_secret_like("AUTH_TOKEN"));
        assert!(is_secret_like("DB_PASSWORD"));
        assert!(is_secret_like("PRIVATE_KEY"));
        assert!(!is_secret_like("NODE_ENV"));
        assert!(!is_secret_like("PUBLIC_API_KEY"));
        assert!(!is_secret_like("NX_TOKEN"));
        assert!(!is_secret_like("npm_TOKEN"));
        assert!(!is_secret_like("APP_NAME"));
    }
}
