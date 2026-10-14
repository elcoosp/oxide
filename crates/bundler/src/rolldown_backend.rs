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
    Platform, RawMinifyOptions, SourceMapType, TsConfig,
};
use rolldown_common::{ModuleType, Output};
use rolldown_plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookUsage, Plugin, Pluginable,
    SharedLoadPluginContext,
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

        BundlerOptions {
            input: Some(input),
            cwd: Some(self.root.clone()),
            platform: Some(platform),
            format: Some(OutputFormat::Esm),
            transform,
            // `dir` = output directory; Rolldown's `write()` persists files there.
            dir: Some(options.out_dir.to_string_lossy().into_owned()),
            sourcemap,
            minify,
            external,
            define,
            tsconfig,
            ..Default::default()
        }
    }
}

impl Bundler for RolldownBundler {
    fn bundle(&self, options: &BundleOptions) -> crate::Result<BundleOutput> {
        let ropts = self.build_options(options);
        let out_dir = options.out_dir.clone();
        let env = options.env;
        let plugins = precompiled_plugins(options);

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

fn precompiled_plugins(options: &BundleOptions) -> Vec<std::sync::Arc<Pluginable>> {
    if options.precompiled.is_empty() {
        Vec::new()
    } else {
        vec![Pluginable::new_shared(PrecompiledPlugin::new(options.precompiled.clone()))]
    }
}
