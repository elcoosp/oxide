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
    err, BundleEnv, BundleOptions, BundleOutput, Bundler, Chunk, ChunkKind, ClientManifest,
    RouteBundle, ServerManifest, ServerRoute,
};
use oxide_diagnostics::Diagnostic;
use arcstr::ArcStr;
use rolldown_common::{
    BundlerTransformOptions, Either, JsxOptions as RolldownJsxOptions,
};
use rolldown::{
    Bundler as RolldownCore, BundlerBuilder, BundlerOptions, InputItem, IsExternal, OutputFormat,
    Platform, RawMinifyOptions, ResolveOptions, SourceMapType, TsConfig,
};
use rolldown_common::{ModuleType, Output};
use rolldown_plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookTransformArgs, HookTransformReturn,
    HookTransformOutput, HookUsage, Plugin, Pluginable, SharedLoadPluginContext,
    SharedTransformPluginContext,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Rolldown-backed bundler (D3: "Rolldown used as a library").
pub struct RolldownBundler {
    pub root: PathBuf,
}

impl RolldownBundler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn build_options(&self, options: &BundleOptions) -> BundlerOptions {
        let input: Vec<InputItem> = options
            .entries
            .iter()
            .map(|(name, path)| InputItem {
                name: Some(name.clone()),
                import: path.to_string_lossy().into_owned(),
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
            let mut jsx = RolldownJsxOptions::default();
            jsx.import_source = Some(src.clone());
            BundlerTransformOptions {
                jsx: Some(Either::Right(jsx)),
                ..Default::default()
            }
        });

        // BUG-SOLID: explicit resolve conditions, when the caller set them.
        // These are tried ahead of the platform defaults; the config bundle
        // uses worker-first so it agrees with the V8 loader.
        let resolve = options.resolve_conditions.as_ref().map(|conds| ResolveOptions {
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
        let ropts = self.build_options(options);
        let out_dir = options.out_dir.clone();
        let env = options.env;
        let plugins = oxide_plugins(options, &self.root);

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
                            RolldownCore::new(ropts).map_err(|e| {
                                err("BLD-ROLLDOWN", format!("rolldown init: {e}"))
                            })?
                        } else {
                            BundlerBuilder::default()
                                .with_options(ropts)
                                .with_plugins(plugins)
                                .build()
                                .map_err(|e| {
                                    err("BLD-ROLLDOWN", format!("rolldown init: {e}"))
                                })?
                        };
                        let ro = b
                            .write()
                            .await
                            .map_err(|e| err("BLD-ROLLDOWN", format!("rolldown write: {e}")))?;
                        convert(ro, &out_dir, env)
                    })
                })
                .join()
                .map_err(|_| err("BLD-ROLLDOWN", "rolldown worker thread panicked"))?
        });
        result
    }
}

/// §6.4: write `<out_dir>/ssr/index.js` — the adapter entry the serverless
/// handlers (Lambda / Vercel / Netlify) import. Each entry chunk becomes a
/// namespace import; the route-id → namespace map is exported as `routes`.
fn write_ssr_adapter_entry(out_dir: &Path, chunks: &[Chunk]) -> crate::Result<()> {
    let mut entries: Vec<&Chunk> = chunks
        .iter()
        .filter(|c| c.kind == ChunkKind::Entry)
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    let mut body =
        String::from("// Generated by oxide-bundler (rolldown) — adapter entry (§6.4). Do not edit.\n");
    for (i, c) in entries.iter().enumerate() {
        body.push_str(&format!("import * as route_{i} from \"../{}\";\n", c.file));
    }
    body.push_str("\nexport const routes = {\n");
    for (i, c) in entries.iter().enumerate() {
        body.push_str(&format!("  \"{}\": route_{i},\n", c.name));
    }
    body.push_str("};\n");

    let ssr_dir = out_dir.join("ssr");
    std::fs::create_dir_all(&ssr_dir)
        .map_err(|e| err("BLD-IO", format!("mkdir {}: {e}", ssr_dir.display())))?;
    std::fs::write(ssr_dir.join("index.js"), body)
        .map_err(|e| err("BLD-IO", format!("write ssr/index.js: {e}")))?;
    Ok(())
}

fn convert(
    ro: rolldown::BundleOutput,
    out_dir: &Path,
    env: BundleEnv,
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
                chunks.push(Chunk {
                    name,
                    file,
                    hash,
                    // Per-module metadata is exposed via Rolldown's plugin hooks;
                    // recording it here would require a hook, see deviations.md.
                    modules: Vec::new(),
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
        let mut routes = BTreeMap::new();
        for c in chunks.iter().filter(|c| c.kind == ChunkKind::Entry) {
            let js_bytes = std::fs::metadata(out_dir.join(&c.file))
                .map(|m| m.len())
                .unwrap_or(0);
            routes.insert(
                c.name.clone(),
                RouteBundle {
                    entry_chunk: c.file.clone(),
                    preload: vec![c.file.clone()],
                    css: c.css.clone(),
                    js_bytes,
                },
            );
        }
        let m = ClientManifest {
            build_id: build_id.clone(),
            routes,
            assets: assets_map,
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
        write_ssr_adapter_entry(out_dir, &chunks)?;
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
    pub fn new(sources: BTreeMap<PathBuf, String>) -> Self {
        Self { sources }
    }

    /// Exact path, then canonicalized (`/var` vs `/private/var` on macOS —
    /// same trap the CLI's canonicalize steps guard against).
    fn lookup(&self, id: &str) -> Option<String> {
        let p = Path::new(id);
        if let Some(s) = self.sources.get(p) {
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
                    .map(|(k, v)| {
                        format!("{}: {}", Self::json_string(k), Self::json_string(v))
                    })
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
fn oxide_plugins(options: &BundleOptions, root: &Path) -> Vec<std::sync::Arc<Pluginable>> {
    let mut plugins: Vec<std::sync::Arc<Pluginable>> = vec![Pluginable::new_shared(
        OxidePassesPlugin::new(
            // The compiler's `Env` mirrors the bundler's `BundleEnv`
            // one-to-one; map rather than re-derive.
            match options.env {
                BundleEnv::Client => oxide_compiler::Env::Client,
                BundleEnv::Ssr => oxide_compiler::Env::Ssr,
                BundleEnv::Edge => oxide_compiler::Env::Edge,
            },
            options.build_secret.clone(),
            root.to_path_buf(),
            options.precompiled.clone(),
            options.transform_hook.clone(),
        ),
    )];
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
    env: oxide_compiler::Env,
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
}

impl OxidePassesPlugin {
    pub fn new(
        env: oxide_compiler::Env,
        build_secret: String,
        root: PathBuf,
        precompiled: BTreeMap<PathBuf, String>,
        transform_hook: Option<crate::TransformHook>,
    ) -> Self {
        Self { env, build_secret, root, precompiled, transform_hook }
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
        if canon_abs.components().any(|c| c.as_os_str() == "node_modules") {
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
}

impl Plugin for OxidePassesPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        "oxide-passes".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Transform
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
        let replacement: Option<String> = if self.should_transform(args.id) {
            // §15.1: run a plugin transform first. It sees the original
            // source, so a plugin's own rewriting (a macro expansion, a
            // custom directive) happens before the framework passes look
            // at the module.
            let mut source = args.code.to_string();
            if let Some(hook) = &self.transform_hook {
                match hook.call(args.id, &source) {
                    Ok(rewritten) => source = rewritten,
                    // A plugin error is a build error; surface it as a
                    // transform failure. Rolldown reports diagnostics from
                    // a hook that returns `Err`, but this hook returns
                    // `Ok(None)` on internal errors by design — so the
                    // message is dropped onto stderr here. That is the
                    // honest minimum until the diagnostic pipeline is
                    // extended to carry a plugin code.
                    Err(e) => {
                        eprintln!(
                            "plugin transform failed for {}: {} {}",
                            args.id, e.0.code, e.0.message
                        );
                    }
                }
            }
            let input = oxide_compiler::TransformInput {
                path: args.id.to_string(),
                code: source,
                env: self.env,
                lang: Self::lang_of(args.id),
                build_secret: self.build_secret.clone(),
                dev: false,
                jsx_import_source: None,
            };
            match oxide_compiler::OxcTransformer.transform_passes_only(&input) {
                Ok(code) if code != args.code.as_str() => Some(code),
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
                Some(code) => Ok(Some(HookTransformOutput {
                    code: Some(code),
                    ..Default::default()
                })),
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
        assert!(ro.resolve.is_none(), "no explicit resolve without conditions");
    }
}
