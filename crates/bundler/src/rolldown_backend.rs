//! Rolldown-backed `Bundler` implementation (spec D3).
//!
//! `RolldownBundler` drives [`rolldown::Bundler`] in-process: it constructs a
//! `BundlerOptions` from the pipeline's [`BundleOptions`], runs `write().await`
//! on a dedicated single-thread Tokio runtime (Rolldown is async and requires
//! one), then adapts the returned `rolldown::BundleOutput` to Oxide's
//! [`BundleOutput`] / [`Chunk`] / manifest shapes.
//!
//! The in-house [`crate::ChunkBundler`] remains available (the trait is the
//! isolation point, so either may be selected by the caller).

use crate::{
    BundleEnv, BundleOptions, BundleOutput, Bundler, Chunk, ChunkKind, ClientManifest, RouteBundle,
    ServerManifest, ServerRoute, err,
};
use arcstr::ArcStr;
use oxide_fw_diagnostics::Diagnostic;
use rolldown::{
    Bundler as RolldownCore, BundlerBuilder, BundlerOptions, InputItem, IsExternal, OutputFormat,
    Platform, RawMinifyOptions, ResolveOptions, SourceMapType, TsConfig,
};
use rolldown_common::{BundlerTransformOptions, Either, JsxOptions as RolldownJsxOptions};
use rolldown_common::{ModuleType, Output};
use rolldown_plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookResolveIdArgs, HookResolveIdOutput,
    HookResolveIdReturn, HookTransformArgs, HookTransformOutput, HookTransformOutputMap,
    HookTransformReturn, HookUsage, Plugin, PluginContext, Pluginable, SharedLoadPluginContext,
    SharedTransformPluginContext,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Rolldown-backed bundler (D3: "Rolldown used as a library").
pub struct RolldownBundler {
    pub root: PathBuf,
    /// Lazy incremental worker. `Mutex` because the trait's `&self` methods
    /// need interior mutability to lazily spawn the worker on first use.
    incremental: std::sync::Mutex<Option<IncrementalWorker>>,
}

/// Strip the Windows extended-length path prefix from `path` and return
/// the simplified form. On Windows, `std::fs::canonicalize()` returns
/// paths starting with `\\?\` (formatted with forward slashes as
/// `//?/C:/...` by rolldown's error reporting), which rolldown's
/// internal resolver treats as an unknown URL scheme and rejects with:
///
/// ```text
/// Cannot resolve entry module //?/C:/.../page.tsx
/// ```
///
/// The verbatim prefix exists to bypass the 260-char path limit; for
/// paths under that limit the non-verbatim form is equivalent and
/// works everywhere. `\\?\C:\foo` -> `C:\foo`,
/// `\\?\UNC\server\share\foo` -> `\\server\share\foo`. Also handles the
/// forward-slash spellings (`//?/C:/foo` -> `C:/foo`). No-op on Unix.
fn strip_windows_verbatim(path: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    // Backslash form (native Windows separators).
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Cow::Owned(format!(r"\\{rest}"));
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return Cow::Borrowed(rest);
    }
    // Forward-slash form (path.to_string_lossy of a Windows path that
    // already used `/`, or a `//?/` URI-ish spelling from rolldown).
    if let Some(rest) = path.strip_prefix("//?/UNC/") {
        return Cow::Owned(format!("//{rest}"));
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        return Cow::Borrowed(rest);
    }
    Cow::Borrowed(path)
}

impl RolldownBundler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            incremental: std::sync::Mutex::new(None),
        }
    }

    fn build_options(&self, options: &BundleOptions) -> BundlerOptions {
        let input: Vec<InputItem> = options
            .entries
            .iter()
            .map(|(name, path)| {
                // Windows extended-length paths (`\\?\C:\...` from
                // `std::fs::canonicalize()`, seen by rolldown as
                // `//?/C:/...`) are treated as an unknown URL scheme by
                // rolldown's resolver and rejected with "Cannot resolve
                // entry module". Strip the verbatim prefix (and its UNC
                // variants) before handing the path to rolldown. No-op
                // on Unix.
                let raw = path.to_string_lossy();
                InputItem {
                    name: Some(name.clone()),
                    import: strip_windows_verbatim(&raw).into_owned(),
                }
            })
            .collect();

        let platform = match options.env {
            BundleEnv::Client | BundleEnv::Edge => Platform::Browser,
            BundleEnv::Ssr => Platform::Node,
        };

        let sourcemap = if options.sourcemap {
            Some(SourceMapType::File)
        } else {
            None
        };
        let minify = if options.minify {
            Some(RawMinifyOptions::Bool(true))
        } else {
            None
        };

        // Externals: `IsExternal: From<Vec<String>>` gives us the string-matching
        // variant without touching `rolldown_utils::pattern_filter::StringOrRegex`
        // (which isn't re-exported by rolldown's public API).
        let external = if options.externals.is_empty() {
            None
        } else {
            Some(IsExternal::from(options.externals.clone()))
        };

        // Defines: textual substitutions performed before tree-shaking.
        let define = if options.define.is_empty() {
            None
        } else {
            Some(
                options
                    .define
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        };

        // tsconfig_paths: Rolldown only accepts `TsConfig::Manual(PathBuf)` — a path
        // to an actual tsconfig.json on disk. If the pipeline supplied paths but no
        // tsconfig file exists, we materialise one in `out_dir` (cleaned up by the
        // caller if desired). This preserves aliasing semantics end-to-end.
        let tsconfig = if options.tsconfig_paths.is_empty() {
            None
        } else {
            let candidate = self.root.join("tsconfig.json");
            if candidate.is_file() {
                Some(TsConfig::Manual(candidate))
            } else {
                // Synthesise a tsconfig.json carrying the `paths` map. It must live
                // beside the project root so `baseUrl: "."` resolves the alias
                // patterns against the actual source tree (not `out_dir`).
                let synthetic = self.root.join(".oxide-tsconfig.json");
                let compiler_options = serde_json::json!({
                    "baseUrl": ".",
                    "paths": options.tsconfig_paths,
                });
                let body = serde_json::json!({
                    "compilerOptions": compiler_options,
                });
                if let Some(parent) = synthetic.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(
                    &synthetic,
                    serde_json::to_vec_pretty(&body).unwrap_or_default(),
                )
                .is_ok()
                {
                    Some(TsConfig::Manual(synthetic))
                } else {
                    None
                }
            }
        };

        // §6.3: when the adapter declares a non-default JSX runtime (Solid
        // 2.0's `@solidjs/web`), tell Rolldown's Oxc transform to import
        // from it. Without this Rolldown lowers `.tsx` against React's
        // runtime — the chunk bundles, but the elements are React
        // elements and Solid's SSR produces an empty string.
        let transform = options.jsx_import_source.as_ref().map(|src| {
            let jsx = RolldownJsxOptions {
                import_source: Some(src.clone()),
                ..Default::default()
            };
            BundlerTransformOptions {
                jsx: Some(Either::Right(jsx)),
                ..Default::default()
            }
        });

        // BUG-SOLID: explicit resolve conditions, when the caller set them.
        // These are tried ahead of the platform defaults; the config bundle
        // uses worker-first so it agrees with the V8 loader.
        let resolve = options
            .resolve_conditions
            .as_ref()
            .map(|conds| ResolveOptions {
                condition_names: Some(conds.clone()),
                ..Default::default()
            });

        BundlerOptions {
            input: Some(input),
            cwd: Some(self.root.clone()),
            platform: Some(platform),
            resolve,
            format: Some(OutputFormat::Esm),
            transform,
            // `dir` = output directory; Rolldown's `write()` persists files there.
            dir: Some(options.out_dir.to_string_lossy().into_owned()),
            sourcemap,
            minify,
            external,
            define,
            tsconfig,
            // §4.2: a config/SSR bundle for a bare V8 host must be one file —
            // `code_splitting: false` inlines dynamic imports so a CJS runtime
            // (react) is evaluated in place instead of landing in a chunk that
            // re-exports the `require` wrapper.
            code_splitting: if options.inline_dynamic_imports {
                Some(rolldown_common::CodeSplittingMode::Bool(false))
            } else {
                None
            },
            ..Default::default()
        }
    }
}

impl Bundler for RolldownBundler {
    fn bundle(&self, options: &BundleOptions) -> crate::Result<BundleOutput> {
        // A cold `bundle` call invalidates any incremental state — the
        // caller asked for a full rebuild, so give them exactly that.
        if let Ok(mut guard) = self.incremental.lock() {
            *guard = None;
        }
        self.bundle_cold(options)
    }

    fn bundle_incremental(
        &self,
        options: &BundleOptions,
        changed: Option<&[PathBuf]>,
    ) -> crate::Result<BundleOutput> {
        let desired_hash = hash_bundle_options(options);
        let mut guard = self.incremental.lock().unwrap_or_else(|p| p.into_inner());

        let needs_new = match guard.as_ref() {
            None => true,
            Some(w) => w.options_hash != desired_hash,
        };
        if needs_new {
            *guard = None;
            *guard = Some(IncrementalWorker::start(self, options)?);
        }
        guard.as_ref().unwrap().rebuild(options, changed)
    }
}

impl RolldownBundler {
    fn bundle_cold(&self, options: &BundleOptions) -> crate::Result<BundleOutput> {
        let ropts = self.build_options(options);
        let out_dir = options.out_dir.clone();
        // The scoped closure consumes `out_dir`; the post-emit hook
        // pass below needs its own handle.
        let out_dir_for_hooks = out_dir.clone();
        let env = options.env;
        // The plugin sink: a failed Oxc pass records the full diagnostic
        // (location, code frame, fix) here, since Rolldown's hook error
        // channel only carries a string. Drained into the result below.
        let diagnostics_sink: std::sync::Arc<std::sync::Mutex<Vec<Diagnostic>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let plugins = oxide_plugins_with_sink(
            options,
            &self.root,
            std::sync::Arc::clone(&diagnostics_sink),
        );
        // SPEC-15.2: clone the post-emit hooks out of `options` so the
        // scoped thread closure captures owned handles rather than a
        // borrow of `BundleOptions`.
        let render_chunk_hook = options.render_chunk_hook.clone();
        let generate_bundle_hook = options.generate_bundle_hook.clone();

        // Rolldown is async; run it on a dedicated current-thread runtime on a
        // scoped thread so this works regardless of the caller's runtime context.
        let result = std::thread::scope(|scope| {
            scope
                .spawn(move || -> crate::Result<BundleOutput> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| err("BLD-ROLLDOWN", format!("tokio runtime: {e}")))?;
                    rt.block_on(async move {
                        // BundlerBuilder lets us pass plugins alongside options.
                        // The plugin list may be empty (pure JS/TS/TSX project);
                        // in that case we go through the direct constructor.
                        let mut b = if plugins.is_empty() {
                            RolldownCore::new(ropts)
                                .map_err(|e| err("BLD-ROLLDOWN", format!("rolldown init: {e}")))?
                        } else {
                            BundlerBuilder::default()
                                .with_options(ropts)
                                .with_plugins(plugins)
                                .build()
                                .map_err(|e| err("BLD-ROLLDOWN", format!("rolldown init: {e}")))?
                        };
                        let ro = b
                            .write()
                            .await
                            .map_err(|e| err("BLD-ROLLDOWN", format!("rolldown write: {e}")))?;
                        convert(ro, &out_dir, env, &options.entries)
                    })
                })
                .join()
                .map_err(|_| err("BLD-ROLLDOWN", "rolldown worker thread panicked"))?
        });

        // A plugin pass that failed recorded the structured diagnostic (with
        // location, code frame and fix) in the sink. Rolldown's hook error
        // channel carries only a string, so the generic `BLD-ROLLDOWN` wrapper
        // has none of that. When the sink has an entry, surface it as the
        // error — the caller persists diagnostics from the error's own
        // `Diagnostic`, so the frame reaches `.oxide/last-build-diagnostics.json`.
        let plugin_failure: Option<Diagnostic> = diagnostics_sink
            .lock()
            .ok()
            .and_then(|sink| sink.first().cloned());
        if let Some(d) = plugin_failure {
            return Err(crate::OxideError(Box::new(d)));
        }

        // SPEC-15.2: run `renderChunk` over every emitted chunk file, then
        // `generateBundle` once over the final file list. Rolldown's `write()`
        // has already persisted the files, so a `renderChunk` hook rewrites them
        // in place. Both hooks compose with the caller's `BundleOptions` — a
        // hook that returns `None` leaves the chunk untouched.
        let mut result = result?;
        if let Some(hook) = render_chunk_hook.as_ref() {
            for c in &result.chunks {
                let file_path = out_dir_for_hooks.join(&c.file);
                let Ok(src) = std::fs::read_to_string(&file_path) else {
                    continue;
                };
                if let Ok(Some(new_src)) = hook.call(&c.file, &src)
                    && new_src != src
                {
                    let _ = std::fs::write(&file_path, &new_src);
                }
            }
        }
        if let Some(hook) = generate_bundle_hook.as_ref() {
            let files: Vec<String> = result.chunks.iter().map(|c| c.file.clone()).collect();
            // The hook may drop or reorder files. `BundleOutput` reports chunks,
            // not a flat file list — this lets a plugin veto a chunk from the
            // manifests without inventing a chunk the bundler never produced.
            if let Ok(Some(kept)) = hook.call(&files) {
                let keep: std::collections::BTreeSet<&str> =
                    kept.iter().map(String::as_str).collect();
                result.chunks.retain(|c| keep.contains(c.file.as_str()));
            }
        }
        Ok(result)
    }
}

/// §6.4: write `<out_dir>/ssr/index.js` — the adapter entry the serverless
/// handlers (Lambda / Vercel / Netlify) import. Each entry chunk becomes a
/// namespace import; the route-id → namespace map is exported as `routes`.
fn write_ssr_adapter_entry(
    out_dir: &Path,
    chunks: &[Chunk],
    route_entries: &BTreeMap<String, PathBuf>,
) -> crate::Result<()> {
    // §6.4 / WP-A2: role = an entry chunk. Each chunk carries `modules` —
    // the source files that went into it. Look up the route id by finding
    // a module path whose `ends_with` matches the entries map's source
    // path (canonicalization may differ, so ends_with, not equality).
    // Rolldown names chunks after the sanitized file basename
    // (`[slug]` -> `_slug_`, `/page` stripped); the pipeline addresses
    // routes in their source form. Emitting the source form here is the
    // structural fix — the earlier runtime alias in the worker becomes a
    // compatibility shim for pre-WP-A2 builds, not the load-bearing path.
    let mut source_to_id: BTreeMap<&Path, String> = BTreeMap::new();
    for (id, src) in route_entries {
        let canonical = id.strip_suffix("/page").unwrap_or(id).to_string();
        source_to_id.insert(src.as_path(), canonical);
    }
    let resolve_route_id = |c: &Chunk| -> Option<String> {
        for module in &c.modules {
            // Direct hit (canonicalized identically).
            if let Some(id) = source_to_id.get(module.as_path()) {
                return Some(id.clone());
            }
            // Fallback: prefix-stable ends_with. Rolldown's module id is a
            // path (may have a symlink prefix); the route source path is
            // what we recorded. Both end at the same file.
            for (src, id) in &source_to_id {
                if module.ends_with(src) {
                    return Some(id.clone());
                }
            }
        }
        None
    };

    let mut entry_chunks: Vec<&Chunk> = chunks
        .iter()
        .filter(|c| c.kind == ChunkKind::Entry)
        .collect();
    entry_chunks.sort_by(|a, b| a.name.cmp(&b.name));

    let ssr_dir = out_dir.join("ssr");
    std::fs::create_dir_all(&ssr_dir)
        .map_err(|e| err("BLD-IO", format!("mkdir {}: {e}", ssr_dir.display())))?;

    // §6.4 eager entry — every route module imported as a side effect.
    // Consumed by `server-fn:run` and `action:run`: those ops need every
    // route module's `__oxide_register_fn`/action registry populated
    // before first dispatch, and that registration happens at import
    // time. Eager is the correct shape here.
    let mut body = String::from(
        "// Generated by oxide-fw-bundler (rolldown) — adapter entry (§6.4). Do not edit.\n",
    );
    for (i, c) in entry_chunks.iter().enumerate() {
        body.push_str(&format!("import * as route_{i} from \"../{}\";\n", c.file));
    }
    body.push_str("\nexport const routes = {\n");
    for (i, c) in entry_chunks.iter().enumerate() {
        let route_id = resolve_route_id(c).unwrap_or_else(|| c.name.clone());
        body.push_str(&format!("  \"{route_id}\": route_{i},\n"));
    }
    body.push_str("};\n");
    std::fs::write(ssr_dir.join("index.js"), body)
        .map_err(|e| err("BLD-IO", format!("write ssr/index.js: {e}")))?;

    // §6.4 / Wave M: lazy entry — route modules wrapped in dynamic imports.
    // Importing this file costs a few ms; the render path then pulls only
    // the matched route + its layouts + error boundary, not all 12 modules.
    // The render dispatch (`framework:render`) uses this entry; the eager
    // `index.js` stays for server-fn / action dispatch.
    let mut lazy_body = String::from(
        "// Generated by oxide-fw-bundler (rolldown) — lazy adapter entry (§6.4). Do not edit.\n",
    );
    lazy_body.push_str("\nexport const routes = {\n");
    for c in entry_chunks.iter() {
        let route_id = resolve_route_id(c).unwrap_or_else(|| c.name.clone());
        lazy_body.push_str(&format!(
            "  \"{route_id}\": () => import(\"../{}\"),\n",
            c.file
        ));
    }
    lazy_body.push_str("};\n");
    std::fs::write(ssr_dir.join("lazy.js"), lazy_body)
        .map_err(|e| err("BLD-IO", format!("write ssr/lazy.js: {e}")))?;

    Ok(())
}

fn convert(
    ro: rolldown::BundleOutput,
    out_dir: &Path,
    env: BundleEnv,
    route_entries: &BTreeMap<String, PathBuf>,
) -> crate::Result<BundleOutput> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut assets_map: BTreeMap<String, String> = BTreeMap::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    for w in ro.warnings {
        diagnostics.push(Diagnostic::error(
            "BLD-ROLLDOWN",
            format!("rolldown warning: {w:?}"),
        ));
    }

    for output in ro.assets {
        match output {
            Output::Chunk(c) => {
                let file = c.filename.to_string();
                let name = c.name.to_string();
                let kind = if c.is_entry {
                    ChunkKind::Entry
                } else {
                    ChunkKind::Shared
                };
                let imports: Vec<String> = c.imports.iter().map(|s| s.to_string()).collect();
                // Hash the persisted file (Rolldown `write()` already saved it).
                let hash = std::fs::read(out_dir.join(&file))
                    .map(|b| blake3::hash(&b).to_hex().to_string()[..12].to_string())
                    .unwrap_or_default();
                // PERF-09: record the modules in this chunk (the resolver's
                // canonical ids). Lets `oxide analyze`/`why` attribute bytes
                // per module and per chunk instead of guessing.
                let modules: Vec<std::path::PathBuf> = c
                    .module_ids
                    .iter()
                    .map(|id| std::path::PathBuf::from(id.to_string()))
                    .collect();
                chunks.push(Chunk {
                    name,
                    file,
                    hash,
                    modules,
                    imports,
                    css: Vec::new(),
                    kind,
                });
            }
            Output::Asset(a) => {
                let file = a.filename.to_string();
                for orig in &a.original_file_names {
                    assets_map.insert(orig.clone(), file.clone());
                }
                if file.ends_with(".css") {
                    for c in chunks.iter_mut() {
                        if !c.css.contains(&file) {
                            c.css.push(file.clone());
                        }
                    }
                }
            }
        }
    }

    chunks.sort_by(|a, b| a.name.cmp(&b.name));

    let joined = chunks
        .iter()
        .map(|c| c.hash.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let build_id = blake3::hash(joined.as_bytes()).to_hex().to_string()[..16].to_string();

    let client_manifest = if env == BundleEnv::Client {
        // PERF-09: route-true byte accounting. A route's real cost is its
        // entry chunk PLUS every chunk transitively reachable through the
        // import graph (shared chunks the route actually loads). The old
        // entry-only number under-reported every route that shares code.
        let size_of = |file: &str| -> u64 {
            std::fs::metadata(out_dir.join(file))
                .map(|m| m.len())
                .unwrap_or(0)
        };
        // file name → (imports as file names, own bytes)
        let by_name: std::collections::HashMap<String, (&Vec<String>, u64)> = chunks
            .iter()
            .map(|c| (c.name.clone(), (&c.imports, size_of(&c.file))))
            .collect();
        // A chunk's imports are chunk *names*; map name → file for the walk.
        let file_of: std::collections::HashMap<String, String> = chunks
            .iter()
            .map(|c| (c.name.clone(), c.file.clone()))
            .collect();
        let reachable_bytes = |entry_name: &str| -> u64 {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut stack = vec![entry_name.to_string()];
            let mut total = 0u64;
            while let Some(n) = stack.pop() {
                if !seen.insert(n.clone()) {
                    continue;
                }
                if let Some((imports, bytes)) = by_name.get(&n) {
                    total += bytes;
                    for i in imports.iter() {
                        // Imports may be a name or a file; normalize via file_of.
                        let child = file_of
                            .iter()
                            .find(|(_, f)| f.as_str() == i.as_str())
                            .map(|(name, _)| name.clone())
                            .unwrap_or_else(|| i.clone());
                        stack.push(child);
                    }
                }
            }
            total
        };
        let mut routes = BTreeMap::new();
        for c in chunks.iter().filter(|c| c.kind == ChunkKind::Entry) {
            let js_bytes = reachable_bytes(&c.name);
            // PERF-09: the reachable module list (same walk as the byte sum).
            let mut mods: Vec<PathBuf> = Vec::new();
            {
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut stack = vec![c.name.clone()];
                while let Some(n) = stack.pop() {
                    if !seen.insert(n.clone()) {
                        continue;
                    }
                    if let Some(ch) = chunks.iter().find(|ch| ch.name == n) {
                        mods.extend(ch.modules.iter().cloned());
                        for i in ch.imports.iter() {
                            let child = file_of
                                .iter()
                                .find(|(_, f)| f.as_str() == i.as_str())
                                .map(|(name, _)| name.clone())
                                .unwrap_or_else(|| i.clone());
                            stack.push(child);
                        }
                    }
                }
            }
            routes.insert(
                c.name.clone(),
                RouteBundle {
                    entry_chunk: c.file.clone(),
                    preload: vec![c.file.clone()],
                    css: c.css.clone(),
                    js_bytes,
                    modules: mods,
                    layout_chain: Vec::new(),
                },
            );
        }
        let m = ClientManifest {
            build_id: build_id.clone(),
            routes,
            assets: assets_map,
            // V2-E04: sha384 digests for every emitted JS chunk and CSS
            // asset. Rolldown persisted the files already, so the pass reads
            // them back from `out_dir` — the same bytes the browser fetches,
            // hashed exactly once.
            integrity: crate::sri::integrity_map(
                out_dir,
                chunks
                    .iter()
                    .map(|c| c.file.as_str())
                    .chain(chunks.iter().flat_map(|c| c.css.iter().map(String::as_str))),
            ),
        };
        std::fs::write(
            out_dir.join("client.manifest.json"),
            serde_json::to_string_pretty(&m).map_err(|e| err("BLD-JSON", format!("{e}")))? + "\n",
        )
        .map_err(|e| err("BLD-IO", format!("write client manifest: {e}")))?;
        Some(m)
    } else {
        None
    };

    let server_manifest = if env == BundleEnv::Ssr {
        let mut routes = BTreeMap::new();
        for c in chunks.iter().filter(|c| c.kind == ChunkKind::Entry) {
            routes.insert(
                c.name.clone(),
                ServerRoute {
                    module: c.file.clone(),
                    render_mode: None,
                    tags: Vec::new(),
                    prerendered: None,
                    layout_chain: Vec::new(),
                },
            );
        }
        let m = ServerManifest {
            build_id: build_id.clone(),
            routes,
            adapter_entry: "ssr/index.js".to_string(),
        };
        // §6.4: the serverless handler imports `./server/ssr/index.js` and
        // expects it to export a `routes` map from route id → module namespace.
        // Rolldown doesn't generate this on its own — it's our contract with
        // the deploy adapters — so we write it alongside the emitted chunks.
        write_ssr_adapter_entry(out_dir, &chunks, route_entries)?;
        std::fs::write(
            out_dir.join("server.manifest.json"),
            serde_json::to_string_pretty(&m).map_err(|e| err("BLD-JSON", format!("{e}")))? + "\n",
        )
        .map_err(|e| err("BLD-IO", format!("write server manifest: {e}")))?;
        Some(m)
    } else {
        None
    };

    Ok(BundleOutput {
        modules: Vec::new(),
        chunks,
        client_manifest,
        server_manifest,
        diagnostics,
    })
}

/// §6.3: serve precompiled JS for `.vue` / `.svelte` modules.
///
/// The CLI's `precompile.rs` pass runs the tooling pool to lower SFCs and
/// hands the results in via `BundleOptions::precompiled`. This plugin
/// intercepts Rolldown's `load` hook for those paths and returns the JS,
/// marking it `ModuleType::Js` so Rolldown skips trying to parse the raw
/// SFC as JavaScript. Without it the CLI had to fall back to ChunkBundler
/// for any project with an SFC entry, losing Rolldown's sourcemaps.
#[derive(Debug)]
pub struct PrecompiledPlugin {
    sources: BTreeMap<PathBuf, String>,
}

impl PrecompiledPlugin {
    /// BUG-MAPKEY: the keys MUST be lexically normalized. Callers built the
    /// map from lexical joins (`routes/index` + `../../components/a.vue`
    /// keeps the `..` segments), while Rolldown resolves the same import to
    /// the normalized spelling — the lookup missed and the raw SFC reached
    /// the parser. Normalizing on both sides (here and in `lookup`) makes
    /// the comparison spelling-independent.
    pub fn new(sources: BTreeMap<PathBuf, String>) -> Self {
        Self {
            sources: sources
                .into_iter()
                .map(|(k, v)| (crate::normalize(&k), v))
                .collect(),
        }
    }

    /// Exact path, then lexically normalized, then canonicalized (`/var` vs
    /// `/private/var` on macOS — same trap the CLI's canonicalize steps guard
    /// against). The normalized fallback is the BUG-MapKey net: precompile
    /// keys are normalized paths, while an importer may hand Rolldown an id
    /// with `..` segments (`/proj/src/routes/index/../../components/a.vue`);
    /// canonicalize alone resolves those through the filesystem, but only
    /// when the file exists, so the lexical pass must come first.
    fn lookup(&self, id: &str) -> Option<String> {
        let p = Path::new(id);
        if let Some(s) = self.sources.get(p) {
            return Some(s.clone());
        }
        let normalized = crate::normalize(p);
        if let Some(s) = self.sources.get(&normalized) {
            return Some(s.clone());
        }
        let canon = PathBuf::from(id).canonicalize().ok()?;
        self.sources.get(&canon).cloned()
    }
}

impl Plugin for PrecompiledPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-precompiled".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Load
    }

    fn load(
        &self,
        _ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> impl std::future::Future<Output = HookLoadReturn> + Send {
        let src = self.lookup(args.id);
        async move {
            let Some(code) = src else { return Ok(None) };
            Ok(Some(HookLoadOutput {
                code: ArcStr::from(code),
                map: None,
                side_effects: None,
                module_type: Some(ModuleType::Js),
            }))
        }
    }
}

/// §15-E: bridge the CLI-level `resolveId` plugin hook (JS/WASM plugins,
/// wrapped by the CLI into [`ResolveHook`]) into Rolldown's own resolve
/// chain. First plugin returning a concrete id wins; `None` falls through to
/// the built-ins, exactly like the dev-path walker in `bundle_incremental`.
#[derive(Debug)]
pub struct HookResolveIdBridgePlugin {
    resolve_hook: Option<crate::ResolveHook>,
}

impl HookResolveIdBridgePlugin {
    pub fn new(resolve_hook: Option<crate::ResolveHook>) -> Self {
        Self { resolve_hook }
    }
}

impl Plugin for HookResolveIdBridgePlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-hook-resolve-bridge".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }

    fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> impl std::future::Future<Output = HookResolveIdReturn> + Send {
        let hook = self.resolve_hook.clone();
        let specifier = args.specifier.to_string();
        let importer = args.importer.map(str::to_string).unwrap_or_default();
        async move {
            let Some(hook) = hook else { return Ok(None) };
            // The CLI closure blocks on a worker round-trip (its own scoped
            // thread + runtime); that is acceptable here — the bundler runs on
            // a dedicated thread and the plugin worker is a separate process,
            // so a stall cannot deadlock. A hook failure degrades to `None`
            // (fall through to the built-in resolver) and is reported on
            // stderr, matching the CssPlugin's degradation convention.
            match hook.call(&specifier, &importer) {
                Ok(Some(p)) => Ok(Some(HookResolveIdOutput {
                    id: ArcStr::from(p.to_string_lossy().as_ref()),
                    external: None,
                    normalize_external_id: None,
                    side_effects: None,
                    package_json_path: None,
                })),
                Ok(None) => Ok(None),
                Err(e) => {
                    eprintln!(
                        "plugin resolveId({specifier:?}) failed: {} {}",
                        e.0.code, e.0.message
                    );
                    Ok(None)
                }
            }
        }
    }
}

/// §15-E: bridge the CLI-level `load` plugin hook into Rolldown's load chain.
/// The FIRST non-null `code` overrides the disk read (same contract the CLI
/// doc-comment specifies); `None` falls through to the built-ins.
#[derive(Debug)]
pub struct HookLoadBridgePlugin {
    load_hook: Option<crate::LoadHook>,
}

impl HookLoadBridgePlugin {
    pub fn new(load_hook: Option<crate::LoadHook>) -> Self {
        Self { load_hook }
    }
}

impl Plugin for HookLoadBridgePlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-hook-load-bridge".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Load
    }

    fn load(
        &self,
        _ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> impl std::future::Future<Output = HookLoadReturn> + Send {
        let hook = self.load_hook.clone();
        let id = args.id.to_string();
        async move {
            let Some(hook) = hook else { return Ok(None) };
            match hook.call(&id) {
                Ok(Some(code)) => Ok(Some(HookLoadOutput {
                    code: ArcStr::from(code),
                    map: None,
                    side_effects: None,
                    module_type: Some(ModuleType::Js),
                })),
                Ok(None) => Ok(None),
                Err(e) => {
                    eprintln!("plugin load({id:?}) failed: {} {}", e.0.code, e.0.message);
                    Ok(None)
                }
            }
        }
    }
}

/// §6.6: run every `.css` module through `css_transform` (Lightning CSS,
/// wrapped by the CLI) and emit the result as a content-hashed asset.
///
/// Rolldown 1.2.x refuses a `.css` module outright — `module_task.rs`
/// returns `unsupported_feature("Bundling CSS is no longer supported …")`
/// the moment a loaded module's type is `Css`. That check runs *after* the
/// plugin `load` hooks, so a plugin can serve the stylesheet as JavaScript
/// (`ModuleType::Js`) and emit the CSS itself. Without this plugin an
/// `oxide build` on a project with any stylesheet import fails before the
/// bundler produces anything.
#[derive(Debug)]
pub struct CssPlugin {
    css_transform: Option<crate::CssTransformHook>,
}

impl CssPlugin {
    pub fn new(css_transform: Option<crate::CssTransformHook>) -> Self {
        Self { css_transform }
    }

    fn json_string(s: &str) -> String {
        serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
    }
}

impl Plugin for CssPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-css".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Load
    }

    fn load(
        &self,
        ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> impl std::future::Future<Output = HookLoadReturn> + Send {
        // The work is synchronous (a disk read and the transform hook); doing
        // it eagerly and returning a single `async move` keeps the return type
        // one concrete future. Multiple `async move` blocks in one fn do not
        // unify — even identical ones have distinct types.
        let out: Option<HookLoadOutput> = (|| {
            // A rolldown id may carry a query/fragment (`./x.css?inline`);
            // the on-disk path is the part before it.
            let id = args.id.to_string();
            let bare = id.split(['?', '#']).next().unwrap_or(&id).to_string();
            if !Path::new(&bare).extension().is_some_and(|e| e == "css") {
                return None;
            }
            // A missing file is left to rolldown's own loader to report.
            let source = std::fs::read_to_string(&bare).ok()?;
            let is_module = bare.contains(".module.");

            let (code, exports) = match &self.css_transform {
                Some(hook) => match hook.call(&bare, &source, is_module, false) {
                    Ok(v) => v,
                    Err(e) => {
                        // A transform error degrades to verbatim CSS and is
                        // reported on stderr, matching the JS transform hook.
                        eprintln!(
                            "css transform failed for {bare}: {} {}",
                            e.0.code, e.0.message
                        );
                        (source.clone(), BTreeMap::new())
                    }
                },
                None => (source.clone(), BTreeMap::new()),
            };

            // A content-hashed asset name, derived the same way `ChunkBundler`
            // derives its CSS file names, so an unchanged stylesheet lands on
            // the same file across builds.
            let stem = Path::new(&bare)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "style".to_string());
            let hash = crate::ChunkBundler::content_hash(&[code.as_bytes()]);
            let file_name = format!("{stem}-{hash}.css");

            let asset = rolldown_common::EmittedAsset {
                name: Some(file_name.clone()),
                original_file_name: Some(bare.clone()),
                file_name: Some(ArcStr::from(file_name.clone())),
                source: rolldown_common::StrOrBytes::Str(code),
            };
            if let Err(e) = ctx.emit_file(asset, None, None) {
                eprintln!("css emit failed for {bare}: {e}");
            }

            // The JS the importer receives. A plain stylesheet gets a default
            // export carrying the emitted URL; a CSS module also re-exports the
            // local-name → scoped-name map so `import styles from
            // './x.module.css'` works.
            let mut js = String::new();
            if is_module && !exports.is_empty() {
                let entries: Vec<String> = exports
                    .iter()
                    .map(|(k, v)| format!("{}: {}", Self::json_string(k), Self::json_string(v)))
                    .collect();
                js.push_str(&format!("export default {{ {} }};\n", entries.join(", ")));
            } else {
                js.push_str(&format!(
                    "export default {};\n",
                    Self::json_string(&file_name)
                ));
            }

            Some(HookLoadOutput {
                code: ArcStr::from(js),
                map: None,
                side_effects: None,
                module_type: Some(ModuleType::Js),
            })
        })();

        async move { Ok(out) }
    }
}

/// The plugin list for a Rolldown run: the oxide passes (always) and the
/// precompiled-SFC server (only when the CLI supplied precompiled JS).
fn oxide_plugins_with_sink(
    options: &BundleOptions,
    root: &Path,
    diagnostics_sink: std::sync::Arc<std::sync::Mutex<Vec<Diagnostic>>>,
) -> Vec<std::sync::Arc<Pluginable>> {
    let mut plugins: Vec<std::sync::Arc<Pluginable>> =
        vec![Pluginable::new_shared(OxidePassesPlugin::new(
            // The compiler's `Env` mirrors the bundler's `BundleEnv`
            // one-to-one; map rather than re-derive.
            match options.env {
                BundleEnv::Client => oxide_fw_compiler::Env::Client,
                BundleEnv::Ssr => oxide_fw_compiler::Env::Ssr,
                BundleEnv::Edge => oxide_fw_compiler::Env::Edge,
            },
            options.build_secret.clone(),
            root.to_path_buf(),
            options.precompiled.clone(),
            options.transform_hook.clone(),
            diagnostics_sink,
        ))];
    // §6.6: the CSS plugin runs for every project. Rolldown 1.2.x refuses
    // `.css` modules at `module_task.rs`; intercepting them in the `load`
    // hook is the only way an `oxide build` with a stylesheet succeeds.
    plugins.push(Pluginable::new_shared(CssPlugin::new(
        options.css_transform.clone(),
    )));
    if !options.precompiled.is_empty() {
        plugins.push(Pluginable::new_shared(PrecompiledPlugin::new(
            options.precompiled.clone(),
        )));
    }
    // §15-E: bridge the CLI-level `resolveId` / `load` plugin hooks into the
    // production chain. Without this the hooks only run in DEV (the
    // incremental graph walker consults them) — a plugin-virtualized module
    // then resolves in `oxide dev` but the production bundle leaves the
    // import unresolved, Rolldown externalizes the specifier, and the shipped
    // SSR build carries a runtime import of a module that does not exist.
    // LAST in the chain: built-in load/resolve (precompiled, css, fs) keeps
    // priority; the bridge only answers what the built-ins did not.
    if options.resolve_hook.is_some() {
        plugins.push(Pluginable::new_shared(HookResolveIdBridgePlugin::new(
            options.resolve_hook.clone(),
        )));
    }
    if options.load_hook.is_some() {
        plugins.push(Pluginable::new_shared(HookLoadBridgePlugin::new(
            options.load_hook.clone(),
        )));
    }
    plugins
}

/// §6.2 steps 4-6: the framework passes Rolldown's own Oxc lowering does
/// not perform. Rolldown strips TS and lowers JSX; it knows nothing about
/// `'use cache'`, `serverFn(...)` stubs, or the server-only guard — so an
/// SSR chunk built by `RolldownBundler` alone carries a raw
/// `serverFn(async …)` call and no `__oxide_register_fn` line.
///
/// This plugin runs `OxcTransformer::transform_passes_only` on each module
/// *before* Rolldown lowers it: the passes see the original source (exact
/// spans, real identifiers) and the TS/JSX strip still happens once,
/// downstream, in Rolldown.
#[derive(Debug)]
pub struct OxidePassesPlugin {
    env: oxide_fw_compiler::Env,
    build_secret: String,
    /// Absolute project root. A module outside it (a `node_modules`
    /// dependency, a linked workspace package) is skipped — the passes are
    /// for the application's own modules.
    root: PathBuf,
    /// Paths the CLI already precompiled (`solid-jsx-oxc`,
    /// `@vue/compiler-sfc`, `svelte/compiler`). Their emitted JS must not go
    /// through the passes a second time.
    precompiled: BTreeMap<PathBuf, String>,
    /// §15.1: a plugin-supplied `transform` hook (a loaded WASM component,
    /// wrapped as a closure by the CLI). Runs *first* — a plugin sees the
    /// module as its author wrote it, before the `'use cache'` / server-fn
    /// passes rewrite it.
    transform_hook: Option<crate::TransformHook>,
    /// A sink for the *full* diagnostic a failed pass produces. Rolldown's
    /// hook error channel carries only a string, so a diagnostic's location,
    /// code frame and fix would be lost. The plugin records the structured
    /// form here; the bundler drains it into `BundleOutput::diagnostics`,
    /// which the CLI persists.
    diagnostics: std::sync::Arc<std::sync::Mutex<Vec<Diagnostic>>>,
}

impl OxidePassesPlugin {
    pub fn new(
        env: oxide_fw_compiler::Env,
        build_secret: String,
        root: PathBuf,
        precompiled: BTreeMap<PathBuf, String>,
        transform_hook: Option<crate::TransformHook>,
        diagnostics: std::sync::Arc<std::sync::Mutex<Vec<Diagnostic>>>,
    ) -> Self {
        Self {
            env,
            build_secret,
            root,
            // BUG-MAPKEY: normalize here for the same reason
            // `PrecompiledPlugin::new` does — the guard compares against
            // bundler-side ids, which are normalized spellings.
            precompiled: precompiled
                .into_iter()
                .map(|(k, v)| (crate::normalize(&k), v))
                .collect(),
            transform_hook,
            diagnostics,
        }
    }

    /// Should the passes run on the module at `id`?
    fn should_transform(&self, id: &str) -> bool {
        // Rolldown's virtual modules carry a leading NUL (`\0rolldown/…`,
        // `\0vite/…`, and the like). They are not files; joining one onto
        // the root makes it look like a project module that a file check
        // would reject. Skip them outright.
        if id.starts_with('\0') {
            return false;
        }
        let bare = id.split(['?', '#']).next().unwrap_or(id);
        let path = Path::new(bare);
        // Rolldown passes an absolute id for a resolved module in most
        // configurations, but a relative id (resolved against the bundler's
        // `cwd`, which is the project root) is also valid. An absolute path
        // is used as-is; a relative one is joined onto the root.
        let abs: PathBuf = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        // Canonicalize before the containment check: on macOS `/var` is a
        // symlink to `/private/var`, and a resolver may report a path under
        // one spelling while the root was built with the other. Comparing
        // the canonical forms of both sides is the only comparison that
        // holds across the symlink. A path that does not exist cannot be a
        // module to transform, so a failed canonicalization is a skip.
        let canon_root = match self.root.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let canon_abs = match abs.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };
        if !canon_abs.starts_with(&canon_root) {
            return false;
        }
        if canon_abs
            .components()
            .any(|c| c.as_os_str() == "node_modules")
        {
            return false;
        }
        if canon_abs
            .extension()
            .is_some_and(|e| e == "css" || e == "json" || e == "vue" || e == "svelte")
        {
            return false;
        }
        if self.precompiled.contains_key(&canon_abs) || self.precompiled.contains_key(&abs) {
            return false;
        }
        true
    }

    fn lang_of(path: &str) -> &'static str {
        match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("ts") => "ts",
            Some("tsx") => "tsx",
            Some("jsx") => "jsx",
            Some("mjs") | Some("js") => "mjs",
            _ => "mjs",
        }
    }

    /// BUG-MAPKEY guard (pure, unit-testable): does `id` name a framework
    /// SFC (`.vue` / `.svelte`) or `.mdx` document that reached the bundler
    /// WITHOUT precompiled content?
    ///
    /// When the precompile pass misses a module (map-key mismatch, a failed
    /// compile, no tooling pool), the raw file used to flow into Rolldown's
    /// parser — `<template>…` parsed as JSX surfaced as the cryptic
    /// `rolldown write: Unexpected JSX expression`, with no file name. The
    /// guard turns that into a named `BLD-SFC` diagnostic that fails the
    /// build loudly (the sink's first entry becomes the error in
    /// `bundle_cold`).
    fn unprecompiled_sfc(&self, id: &str) -> Option<Diagnostic> {
        // Strip any query/fragment (`./x.vue?raw`), same as the CSS plugin.
        let bare = id.split(['?', '#']).next().unwrap_or(id);
        let ext = Path::new(bare).extension()?.to_str()?;
        if !matches!(ext, "vue" | "svelte" | "mdx") {
            return None;
        }
        let abs = PathBuf::from(bare);
        let normalized = crate::normalize(&abs);
        if self.precompiled.contains_key(&abs) || self.precompiled.contains_key(&normalized) {
            return None;
        }
        // The canonicalize pass resolves symlinked roots (`/var` vs
        // `/private/var`); a miss there settles it.
        if let Ok(canon) = abs.canonicalize()
            && self.precompiled.contains_key(&canon)
        {
            return None;
        }
        let msg = match ext {
            "mdx" => format!(
                "module {bare} reached the bundler as raw Markdown — the MDX precompile pass did not produce content for it"
            ),
            _ => format!(
                "module {bare} reached the bundler as a raw .{ext} file — the precompile pass did not produce content for it"
            ),
        };
        Some(
            Diagnostic::error("BLD-SFC", msg)
                .with_fix(format!(
                    "check the tooling-pool logs for the {ext} compile failure of {bare}; the build must not bundle raw framework SFC sources"
                ))
                .with_cause(
                    "the precompiled-map lookup missed this module id, so Rolldown would parse the raw file (a `<template>` block reads as a JSX expression)",
                ),
        )
    }
}

impl Plugin for OxidePassesPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-passes".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Transform | HookUsage::Load
    }

    /// BUG-MAPKEY guard: a raw `.vue` / `.svelte` / `.mdx` id the precompile
    /// pass never produced content for must fail the build with a named
    /// diagnostic — NOT flow into Rolldown's parser, where `<template>…`
    /// parses as a JSX expression and the build dies with the file-less
    /// `Unexpected JSX expression`. The stub module returned here is only a
    /// parse-safety net: the pushed diagnostic already fails the build.
    fn load(
        &self,
        _ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> impl std::future::Future<Output = HookLoadReturn> + Send {
        // Compute eagerly, return ONE async block — two `async move` blocks in
        // one fn do not unify (see the CssPlugin note below).
        let out: Option<HookLoadOutput> = {
            let violation = self.unprecompiled_sfc(args.id);
            violation.map(|d| {
                if let Ok(mut sink) = self.diagnostics.lock() {
                    sink.push(d);
                }
                HookLoadOutput {
                    code: ArcStr::from(
                        "throw new Error('oxide: unprecompiled framework module (see the BLD-SFC diagnostic)');",
                    ),
                    map: None,
                    side_effects: None,
                    module_type: Some(ModuleType::Js),
                }
            })
        };
        async move { Ok(out) }
    }

    fn transform(
        &self,
        _ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> impl std::future::Future<Output = HookTransformReturn> + Send {
        // The passes are synchronous. Doing the work eagerly (rather than
        // inside the future) keeps this a plain fn whose return type still
        // matches the hook contract.
        // BUG-06: the passes can fail. Carry that as a `Result` so the
        // `Ok(None)`-by-design shape of this hook does not swallow a real
        // failure. `replacement` is computed eagerly (the passes are sync),
        // so the error is produced here and propagated inside the future.
        let mut pass_error: Option<String> = None;
        // V2-C06: the plugin chain's composed map (a V3 JSON string from the
        // hook result's `map` field). Valid only when the passes leave the
        // hook's output byte-identical — the passes produce no map of their
        // own, so once they move positions the plugin's map cannot describe
        // the final code and is withheld.
        let mut plugin_map: Option<String> = None;
        let replacement: Option<(String, Option<String>)> = if self.should_transform(args.id) {
            // §15.1: run a plugin transform first. It sees the original
            // source, so a plugin's own rewriting (a macro expansion, a
            // custom directive) happens before the framework passes look
            // at the module.
            let mut source = args.code.to_string();
            if let Some(hook) = &self.transform_hook {
                match hook.call(args.id, &source) {
                    Ok(out) => {
                        source = out.code;
                        plugin_map = out.map;
                    }
                    // A plugin error is reported and skipped; the build
                    // continues with the module's original source (the
                    // failing-build policy is a deliberate follow-up — see
                    // the `transform_hook_error_does_not_fail_the_build`
                    // pin). The structured event replaces the old bare
                    // `eprintln!`: the code and module id ride the tracing
                    // fields like every other plugin diagnostic.
                    Err(e) => {
                        tracing::warn!(
                            code = %e.0.code,
                            id = %args.id,
                            "plugin transform failed: {}",
                            e.0.message
                        );
                    }
                }
            }
            let input = oxide_fw_compiler::TransformInput {
                path: args.id.to_string(),
                code: source.clone(),
                env: self.env,
                lang: Self::lang_of(args.id),
                build_secret: self.build_secret.clone(),
                dev: false,
                jsx_import_source: None,
            };
            match oxide_fw_compiler::OxcTransformer.transform_passes_only(&input) {
                Ok(code) if code != args.code.as_str() => {
                    // The passes moved positions whenever their output
                    // differs from the hook's; a plugin map describes the
                    // final code only when they did not.
                    let map = if code == source {
                        plugin_map.take()
                    } else {
                        None
                    };
                    Some((code, map))
                }
                // Unchanged output: return `None` so Rolldown keeps its own
                // copy and any downstream map it has already built.
                Ok(_) => None,
                // BUG-06: a pass that *fails* must not be swallowed. The
                // passes include `'use cache'` and server-fn stub
                // transformation — shipping the module untransformed is a
                // correctness hole (a server-only function would leak into
                // the client bundle). Propagate a `CMP-PASS` diagnostic and
                // fail the build loudly.
                Err(e) => {
                    // Record the structured diagnostic (location, code frame,
                    // fix) before flattening it into the string Rolldown's
                    // hook error channel requires. The bundler drains the
                    // sink into `BundleOutput::diagnostics`.
                    if let Ok(mut sink) = self.diagnostics.lock() {
                        sink.push((*e.0).clone());
                    }
                    pass_error = Some(format!("CMP-PASS {}: {}", args.id, e.0.message));
                    None
                }
            }
        } else {
            None
        };

        async move {
            // BUG-06: a failed pass fails the build, loudly.
            if let Some(msg) = pass_error {
                return Err(anyhow::anyhow!("{msg}"));
            }
            match replacement {
                Some((code, map)) => {
                    let map = match map {
                        Some(json) => {
                            match crate::sourcemap_compose::oxc_source_map_from_json(&json) {
                                Ok(sm) => HookTransformOutputMap::Sourcemap(Box::new(sm)),
                                // A malformed plugin map never fails the
                                // build: warn-degrade to an explicit `null`
                                // (the same rule the SVG optimizer and the
                                // font subsetter follow for their refusals).
                                Err(e) => {
                                    tracing::warn!(
                                        code = crate::sourcemap_compose::BLD_SMAP,
                                        id = %args.id,
                                        "plugin map refused: {}",
                                        e.0.message
                                    );
                                    HookTransformOutputMap::Null
                                }
                            }
                        }
                        None => HookTransformOutputMap::Omitted,
                    };
                    Ok(Some(HookTransformOutput {
                        code: Some(code),
                        map,
                        side_effects: None,
                        module_type: None,
                    }))
                }
                None => Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod bug_solid_tests {
    use super::*;
    use std::path::PathBuf;

    fn opts_with(conds: Option<Vec<String>>) -> BundleOptions {
        BundleOptions {
            env: BundleEnv::Client,
            entries: std::collections::BTreeMap::new(),
            out_dir: PathBuf::from("/tmp/oxide-test-out"),
            resolve_conditions: conds,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_conditions_reach_rolldown_options() {
        // BUG-SOLID: the config bundle's worker-first conditions must actually
        // land on `BundlerOptions.resolve.condition_names`, or `@solidjs/web`
        // resolves to its client build (whose `ssr()` returns undefined).
        let b = RolldownBundler::new("/tmp");
        let want = vec![
            "worker".to_string(),
            "browser".to_string(),
            "import".to_string(),
            "module".to_string(),
            "default".to_string(),
        ];
        let ro = b.build_options(&opts_with(Some(want.clone())));
        let got = ro
            .resolve
            .and_then(|r| r.condition_names)
            .expect("resolve.condition_names set");
        assert_eq!(got, want);
    }

    #[test]
    fn no_resolve_conditions_leaves_defaults() {
        // A bundle that does not set conditions keeps rolldown's platform
        // defaults (the pre-BUG-SOLID behavior for every non-config bundle).
        let b = RolldownBundler::new("/tmp");
        let ro = b.build_options(&opts_with(None));
        assert!(
            ro.resolve.is_none(),
            "no explicit resolve without conditions"
        );
    }
}

#[cfg(test)]
mod bug_mapkey_tests {
    //! BUG-MAPKEY: precompiled-map keys and bundler module ids must agree.
    //! The historical failure: `resolve_relative` handed back a key with `..`
    //! segments while Rolldown resolved the same import to the normalized
    //! path; the lookup missed, the raw `.vue`/`.svelte` file reached the
    //! parser, and the build died with the file-less `Unexpected JSX
    //! expression`.

    use super::*;
    use std::collections::BTreeMap;

    fn plugin_with(key: &str) -> PrecompiledPlugin {
        let mut sources = BTreeMap::new();
        sources.insert(PathBuf::from(key), "export default 'compiled'".to_string());
        PrecompiledPlugin::new(sources)
    }

    #[test]
    fn lookup_hits_a_normalized_id_when_the_key_has_dotdot_segments() {
        let p = plugin_with("/proj/src/routes/index/../../components/a.vue");
        assert!(
            p.lookup("/proj/src/components/a.vue").is_some(),
            "bundler-side normalized id must find the non-normalized precompile key"
        );
    }

    #[test]
    fn lookup_hits_a_dotdot_id_when_the_key_is_normalized() {
        let p = plugin_with("/proj/src/components/a.vue");
        assert!(
            p.lookup("/proj/src/routes/index/../../components/a.vue")
                .is_some(),
            "an importer-supplied id with `..` segments must still find the normalized key"
        );
    }

    #[test]
    fn lookup_miss_keeps_failing_for_unknown_modules() {
        let p = plugin_with("/proj/src/components/a.vue");
        assert!(p.lookup("/proj/src/components/other.vue").is_none());
    }

    fn passes_with(keys: &[&str]) -> OxidePassesPlugin {
        let mut precompiled = BTreeMap::new();
        for k in keys {
            precompiled.insert(PathBuf::from(k), "export default 'compiled'".to_string());
        }
        OxidePassesPlugin::new(
            oxide_fw_compiler::Env::Client,
            "secret".to_string(),
            PathBuf::from("/proj"),
            precompiled,
            None,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        )
    }

    #[test]
    fn guard_flags_a_raw_sfc_not_in_the_map() {
        let passes = passes_with(&[]);
        let d = passes
            .unprecompiled_sfc("/proj/src/components/a.vue")
            .expect("raw .vue without precompiled content must be flagged");
        assert_eq!(d.code, "BLD-SFC");
        assert!(d.message.contains("/proj/src/components/a.vue"));
        assert!(
            d.message.contains("raw .vue"),
            "message names the file kind: {}",
            d.message
        );
    }

    #[test]
    fn guard_flags_a_raw_svelte_and_mdx_too() {
        let passes = passes_with(&[]);
        assert!(passes.unprecompiled_sfc("/proj/src/b.svelte").is_some());
        let d = passes
            .unprecompiled_sfc("/proj/content.mdx")
            .expect("raw .mdx must be flagged");
        assert!(d.message.contains("raw Markdown"), "{}", d.message);
    }

    #[test]
    fn guard_passes_a_module_the_map_covers_exactly() {
        let passes = passes_with(&["/proj/src/components/a.vue"]);
        assert!(
            passes
                .unprecompiled_sfc("/proj/src/components/a.vue")
                .is_none()
        );
    }

    #[test]
    fn guard_passes_a_module_the_map_covers_normalized() {
        // The BUG-MAPKEY shape: map key carries `..`, the bundler id does not.
        let passes = passes_with(&["/proj/src/routes/index/../../components/a.vue"]);
        assert!(
            passes
                .unprecompiled_sfc("/proj/src/components/a.vue")
                .is_none(),
            "normalized id must count as covered — the guard must never flag a module lookup would resolve"
        );
    }

    #[test]
    fn guard_passes_a_module_the_map_covers_by_canonical_path() {
        // A symlinked root (`/var` vs `/private/var`): only the canonical
        // spelling is in the map. The file must exist for canonicalize to
        // succeed, so build one under a tempdir.
        let tmp = std::env::temp_dir().join("oxide-bug-mapkey-guard-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let sfc = tmp.join("a.vue");
        std::fs::write(&sfc, "<template />").unwrap();
        let mut precompiled = BTreeMap::new();
        precompiled.insert(
            sfc.canonicalize().unwrap(),
            "export default 'compiled'".to_string(),
        );
        let passes = OxidePassesPlugin::new(
            oxide_fw_compiler::Env::Client,
            "secret".to_string(),
            tmp.clone(),
            precompiled,
            None,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        );
        assert!(
            passes
                .unprecompiled_sfc(sfc.to_string_lossy().as_ref())
                .is_none(),
            "canonical map spelling must count as covered"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guard_ignores_non_sfc_extensions_and_queries() {
        let passes = passes_with(&[]);
        assert!(passes.unprecompiled_sfc("/proj/src/page.tsx").is_none());
        assert!(passes.unprecompiled_sfc("/proj/src/style.css").is_none());
        assert!(
            passes
                .unprecompiled_sfc("/proj/src/logo.svg?react")
                .is_none()
        );
    }
}

#[cfg(test)]
mod strip_windows_verbatim_tests {
    use super::strip_windows_verbatim;

    #[test]
    fn unix_path_is_unchanged() {
        assert_eq!(
            strip_windows_verbatim("/tmp/foo/bar.tsx"),
            "/tmp/foo/bar.tsx"
        );
        assert_eq!(
            strip_windows_verbatim("relative/path.js"),
            "relative/path.js"
        );
        assert_eq!(strip_windows_verbatim(""), "");
    }

    #[test]
    fn strips_verbatim_windows_prefix() {
        assert_eq!(
            strip_windows_verbatim(r"\\?\C:\Users\foo\page.tsx"),
            r"C:\Users\foo\page.tsx"
        );
    }

    #[test]
    fn strips_verbatim_unc_prefix() {
        assert_eq!(
            strip_windows_verbatim(r"\\?\UNC\server\share\foo\page.tsx"),
            r"\\server\share\foo\page.tsx"
        );
    }

    #[test]
    fn strips_forward_slash_verbatim_prefix() {
        // Exact form in the CI failure:
        //   Cannot resolve entry module //?/C:/Users/runneradmin/.../page.tsx
        assert_eq!(
            strip_windows_verbatim("//?/C:/Users/runneradmin/AppData/Local/Temp/foo/page.tsx"),
            "C:/Users/runneradmin/AppData/Local/Temp/foo/page.tsx"
        );
    }

    #[test]
    fn strips_forward_slash_unc_prefix() {
        assert_eq!(
            strip_windows_verbatim("//?/UNC/server/share/foo/page.tsx"),
            "//server/share/foo/page.tsx"
        );
    }

    #[test]
    fn unrelated_path_with_question_mark_is_untouched() {
        // A `?` elsewhere in the path must not be mangled — only the
        // exact leading `\\?\` or `//?/` prefix is stripped.
        assert_eq!(strip_windows_verbatim("src/foo?bar.tsx"), "src/foo?bar.tsx");
    }
}

// ---------------------------------------------------------------------------
// Incremental worker (Path A of the DX rescue)
//
// A dedicated OS thread owns a Rolldown `Bundler` + a Tokio runtime. The
// `Bundler` keeps a snapshot of the last build, so subsequent calls to
// `incremental_write` with `ScanMode::Partial(changed_files)` skip
// re-parsing unchanged modules.
//
// KNOWN CEILING: rolldown's `incremental_write` still runs `bundle_write`
// (tree-shake + chunk + codegen + sourcemap) on the WHOLE graph, regardless
// of `scan_mode`. On the oxide-website project this delivers ~30% faster
// rebuilds (2000ms cold → ~1450ms warm) — not the 5-10x we hoped. To go
// below ~1.4s requires rolldown's `hmr_stage` API (fetch partial modules,
// re-emit only affected chunks), which is a different interface than
// `incremental_write` and a separate future project. See
// internal-docs if/when we tackle it.
//
// The worker is created lazily on the first `bundle_incremental` call. Its
// lifetime is tied to the owning `RolldownBundler`. When that bundler drops,
// the sender drops, the worker's `recv()` returns `Err`, and the thread
// exits. No explicit shutdown needed.
//
// Post-emit hooks (`renderChunk`, `generateBundle`) are applied on the
// CALLER side after receiving the raw `BundleOutput` — no need to plumb
// them through the worker thread.
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, Ordering};

use rolldown_common::ScanMode;

/// One incremental worker: a Rolldown `Bundler` + Tokio runtime on a
/// dedicated thread, driven by messages from the owning `RolldownBundler`.
pub(crate) struct IncrementalWorker {
    request_tx: std::sync::mpsc::Sender<WorkerMsg>,
    /// Fingerprint of the `BundleOptions` this worker was built for. When a
    /// caller's options hash differs (new entry, changed minify, etc.), the
    /// caller drops this worker and builds a new one.
    options_hash: String,
    /// True after the first successful write. Rolldown forces a full scan on
    /// a snapshot-less bundler, so the first request must always be `Full`.
    first_write_done: AtomicBool,
}

enum WorkerMsg {
    Write {
        changed: Option<Vec<arcstr::ArcStr>>,
        out_dir: std::path::PathBuf,
        env: crate::BundleEnv,
        entries: std::collections::BTreeMap<String, std::path::PathBuf>,
        resp_tx: std::sync::mpsc::SyncSender<crate::Result<crate::BundleOutput>>,
    },
}

/// Fingerprint of the options that change the bundler's graph. Hooks are
/// excluded — they're stateless Arc wrappers around plugin state and are
/// applied on the caller side anyway. Plugin *definition* changes show up
/// via `transform_hook` presence or `entries`, which this does catch.
fn hash_bundle_options(options: &crate::BundleOptions) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    format!("{:?}", options.env).hash(&mut h);
    for (k, v) in &options.entries {
        k.hash(&mut h);
        v.to_string_lossy().hash(&mut h);
    }
    options.minify.hash(&mut h);
    options.sourcemap.hash(&mut h);
    for e in &options.externals {
        e.hash(&mut h);
    }
    for (k, v) in &options.define {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    options.build_secret.hash(&mut h);
    options.jsx_import_source.hash(&mut h);
    for (k, v) in &options.precompiled {
        k.to_string_lossy().hash(&mut h);
        v.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

impl IncrementalWorker {
    fn start(bundler: &RolldownBundler, options: &crate::BundleOptions) -> crate::Result<Self> {
        let options_hash = hash_bundle_options(options);
        let ropts = bundler.build_options(options);
        let diagnostics_sink: std::sync::Arc<std::sync::Mutex<Vec<Diagnostic>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        // The plugin ROOT must be the PROJECT root (the cold path passes
        // `&self.root`): `OxidePassesPlugin::should_transform` only runs the
        // 'use cache' / server-fn-stub passes on modules UNDER this root.
        // Passing `options.out_dir` (dist/client) made every project module
        // look out-of-scope, so dev rebuilds shipped `.server` modules into
        // the client bundle unstubbed — `import "node:fs/promises"` reached
        // the browser and the whole module graph died (§6.4 leak, dev-only).
        let plugins = oxide_plugins_with_sink(
            options,
            &bundler.root,
            std::sync::Arc::clone(&diagnostics_sink),
        );

        let (tx, rx) = std::sync::mpsc::channel::<WorkerMsg>();

        std::thread::Builder::new()
            .name("oxide-rolldown-incremental".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::warn!(error = %e, "oxide: incremental worker runtime failed; \
                             incremental rebuilds fall back to full scans");
                        return;
                    }
                };
                let mut bundler = match rt.block_on(async {
                    if plugins.is_empty() {
                        RolldownCore::new(ropts)
                    } else {
                        BundlerBuilder::default()
                            .with_options(ropts)
                            .with_plugins(plugins)
                            .build()
                    }
                }) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "oxide: incremental worker init failed; \
                             incremental rebuilds fall back to full scans");
                        return;
                    }
                };
                while let Ok(WorkerMsg::Write { changed, out_dir, env, entries, resp_tx }) = rx.recv() {
                    let scan_mode: ScanMode<arcstr::ArcStr> = match changed {
                        Some(files) if !files.is_empty() => {
                            tracing::info!(n_files = files.len(), files = ?files, "worker: ScanMode::Partial");
                            ScanMode::Partial(files)
                        }
                        _ => {
                            tracing::info!("worker: ScanMode::Full");
                            ScanMode::Full
                        }
                    };
                    let t0 = std::time::Instant::now();
                    let result = rt.block_on(async { bundler.incremental_write(scan_mode).await });
                    let elapsed = t0.elapsed();
                    tracing::info!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        ok = result.is_ok(),
                        "worker: incremental_write returned"
                    );
                    let converted: crate::Result<crate::BundleOutput> = result
                        .map_err(|e| {
                            err("BLD-ROLLDOWN", format!("incremental write: {e}"))
                        })
                        .and_then(|ro| convert(ro, &out_dir, env, &entries));
                    let _ = resp_tx.send(converted);
                }
            })
            .map_err(|e| err("BLD-ROLLDOWN", format!("spawn incremental worker: {e}")))?;

        Ok(Self {
            request_tx: tx,
            options_hash,
            first_write_done: AtomicBool::new(false),
        })
    }

    fn rebuild(
        &self,
        options: &crate::BundleOptions,
        changed: Option<&[std::path::PathBuf]>,
    ) -> crate::Result<crate::BundleOutput> {
        let files: Option<Vec<arcstr::ArcStr>> = changed.map(|paths| {
            paths
                .iter()
                .map(|p| arcstr::ArcStr::from(p.to_string_lossy().as_ref()))
                .collect()
        });
        // First call is always Full — rolldown enforces this on a
        // snapshot-less bundler regardless, but being explicit here means a
        // caller's first `changed = Some([...])` still gets a full build.
        let is_first = !self.first_write_done.swap(true, Ordering::SeqCst);
        let files = if is_first { None } else { files };

        let (resp_tx, resp_rx) = std::sync::mpsc::sync_channel(1);
        self.request_tx
            .send(WorkerMsg::Write {
                changed: files,
                out_dir: options.out_dir.clone(),
                env: options.env,
                entries: options.entries.clone(),
                resp_tx,
            })
            .map_err(|_| err("BLD-ROLLDOWN", "incremental worker died"))?;
        let mut output = resp_rx
            .recv()
            .map_err(|_| err("BLD-ROLLDOWN", "incremental worker died"))??;

        // Apply post-emit hooks on the caller side (same order as the cold
        // path in `bundle`): renderChunk per chunk, then generateBundle over
        // the final list.
        if let Some(hook) = options.render_chunk_hook.as_ref() {
            for c in &output.chunks {
                let file_path = options.out_dir.join(&c.file);
                if let Ok(src) = std::fs::read_to_string(&file_path)
                    && let Ok(Some(new_src)) = hook.call(&c.file, &src)
                    && new_src != src
                {
                    let _ = std::fs::write(&file_path, &new_src);
                }
            }
        }
        if let Some(hook) = options.generate_bundle_hook.as_ref() {
            let files: Vec<String> = output.chunks.iter().map(|c| c.file.clone()).collect();
            if let Ok(Some(kept)) = hook.call(&files) {
                let keep: std::collections::BTreeSet<&str> =
                    kept.iter().map(String::as_str).collect();
                output.chunks.retain(|c| keep.contains(c.file.as_str()));
            }
        }
        Ok(output)
    }
}
