//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §15.1 kind 2: load the WASM component plugin the config declares, if
/// any, and wrap its `transform` hook as a closure the bundler can call.
///
/// The config's `plugins` array is `Vec<serde_json::Value>` (its shape is
/// S-21.5: resolve a plugin's declared `path` against the project root and
/// confine the result to that root. A plugin entry is config-controlled, but a
/// config can arrive from an untrusted template/PR, and an absolute path or a
/// `..`-escaping relative path would let it load code from anywhere on the
/// host. Absolute paths are refused; a relative path is joined to the
/// canonical root and required to stay under it (symlinks resolved first).
/// Returns `None` (with a warning) when the entry escapes or is unreadable.
pub(crate) fn confine_plugin_path(root: &Path, rel: &str, allow_absolute: bool) -> Option<PathBuf> {
    if Path::new(rel).is_absolute() {
        if allow_absolute {
            return Some(PathBuf::from(rel));
        }
        eprintln!(
            "warn: plugin path {rel:?} is absolute; refusing (set plugins.allowAbsolutePaths = true to permit)"
        );
        return None;
    }
    let root_abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let candidate = root.join(rel);
    // Canonicalize to resolve `..`/symlinks; fall back to the lexical join when
    // the target does not exist (the caller reports "not readable").
    let resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate);
    if !resolved.starts_with(&root_abs) {
        eprintln!("warn: plugin path {rel:?} escapes the project root; refusing");
        return None;
    }
    Some(resolved)
}

/// S-21.5: whether the operator has opted out of plugin path confinement.
/// `plugins` is an array, so the flag is read from any entry that sets
/// `allowAbsolutePaths: true` (a config-wide opt-in).
pub(crate) fn plugins_allow_absolute(cfg: &OxideConfig) -> bool {
    cfg.plugins.iter().any(|p| {
        p.get("allowAbsolutePaths")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    })
}

/// the plugin manifest plus a path). A WASM entry looks like:
///
/// ```text
///     { "kind": "wasm", "path": "./plugins/my-plugin.wasm",
///       "name": "my-plugin", "witWorld": "oxide:plugin/hooks",
///       "capabilities": [], "hooks": ["transform"] }
/// ```
///
/// Returns `None` when no WASM plugin is configured, when the path does not
/// exist, or when the component fails to load — a broken plugin is reported
/// on stderr and the build continues without it, matching how the framework
/// stack degrades. Making a broken plugin fail the build is a policy choice
/// for a follow-up.
/// §6.6: the CSS transform the bundler calls for every emitted stylesheet.
/// Wraps `LightningCssBackend` (Lightning CSS with the project's browser
/// target) so the bundler does not depend on `oxide-fw-assets`.
pub(crate) fn build_css_transform_hook(
    cfg: &OxideConfig,
    tailwind: Option<std::sync::Arc<tailwind::TailwindOutput>>,
) -> oxide_fw_bundler::CssTransformHook {
    use oxide_fw_assets::{CssBackend as _, CssInput, CssKind, LightningCssBackend};

    let targets = vec![cfg.build.target.clone()];
    let tw_entry = tailwind.as_ref().map(|t| t.entry.clone());
    let tw_css = tailwind.as_ref().map(|t| t.css.clone());
    oxide_fw_bundler::CssTransformHook::new(move |path, source, is_module, minify| {
        let effective_source: &str = match (&tw_entry, &tw_css) {
            (Some(entry), Some(css)) if std::path::Path::new(path) == entry.as_path() => {
                css.as_str()
            }
            _ => source,
        };
        let input = CssInput {
            code: effective_source.to_string(),
            file: Some(path.to_string()),
            kind: if is_module {
                CssKind::Module
            } else {
                CssKind::Global
            },
            minify,
            targets: targets.clone(),
        };
        LightningCssBackend
            .transform(&input)
            .map(|out| (out.code, out.exports))
            .map_err(|e| {
                oxide_fw_bundler::err(
                    "BLD-CSS",
                    format!("lightningcss failed for {path}: {}", e.0.message),
                )
            })
    })
}

/// V2-C06: fold one plugin's optional map into the JS chain's accumulated
/// map. `next = None` (the plugin returned no map) means the plugin declares
/// itself position-preserving — the Rollup convention — so the accumulated
/// map keeps describing its output. A map that fails to parse is refused
/// with `BLD-SMAP` and dropped: a broken plugin map never fails a build,
/// but it is never silently accepted either.
pub(crate) fn fold_plugin_map(acc: &mut Option<String>, next: Option<String>) {
    use oxide_fw_bundler::sourcemap_compose::{BLD_SMAP, compose_v3_json, parse_v3};
    let Some(next) = next else {
        return;
    };
    match acc {
        Some(prev) => match compose_v3_json(prev, &next) {
            Ok(composed) => *acc = Some(composed),
            Err(e) => {
                tracing::warn!(code = BLD_SMAP, "plugin map refused: {}", e.0.message);
            }
        },
        None => match parse_v3(&next) {
            Ok(_) => *acc = Some(next),
            Err(e) => {
                tracing::warn!(code = BLD_SMAP, "plugin map refused: {}", e.0.message);
            }
        },
    }
}

/// V2-C06: the JS→WASM hook composition, in one place (it was duplicated at
/// the dev and the build call sites). The WASM stage's protocol carries code
/// only, so by the fold's position-preserving rule the JS chain's composed
/// map keeps describing the combined output.
pub(crate) fn compose_transform_outputs(
    js: oxide_fw_bundler::TransformOutput,
    wasm: oxide_fw_bundler::TransformOutput,
) -> oxide_fw_bundler::TransformOutput {
    oxide_fw_bundler::TransformOutput {
        code: wasm.code,
        map: js.map.or(wasm.map),
    }
}

/// SPEC-17 / §15.1: run the JS plugins' `transform` hook through the tooling
/// worker bridge. Each `kind: js` plugin is imported once (`plugin:load`),
/// then its `transform(code, id)` runs per module. A plugin that does not
/// export `transform` is skipped for that hook.
pub(crate) fn build_js_plugin_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::TransformHook> {
    let pool = pool?;
    let mut plugins: Vec<String> = Vec::new();
    for p in &cfg.plugins {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, plugins_allow_absolute(cfg)) else {
            continue;
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not found; skipping", abs.display());
            continue;
        }
        plugins.push(abs.to_string_lossy().to_string());
    }
    // §15-E: modules declaring `hooks: ["transform"]` contribute their entry
    // to the same chain — a module is a plugin that happens to also ship
    // routes. Each is loaded the same way `plugin:call` loads a kind:js
    // plugin (`<entry>` imported, `<hook>` invoked). `load_quiet` keeps the
    // missing-package / no-contract warnings from printing twice:
    // `scan_routes` already warned once during the route-merge pass.
    for m in crate::modules_loader::load_quiet(root, cfg) {
        if !m.hooks.iter().any(|h| h == "transform") {
            continue;
        }
        let Some(ref entry) = m.entry else {
            eprintln!(
                "warn: module `{}` declares `transform` but has no entry file; skipping",
                m.name,
            );
            continue;
        };
        if !entry.is_file() {
            eprintln!(
                "warn: module `{}` entry {} is missing; skipping transform",
                m.name,
                entry.display(),
            );
            continue;
        }
        plugins.push(entry.to_string_lossy().to_string());
    }
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));

    // The hook is sync; the bundler calls it from inside its own tokio
    // runtime. Calling `Runtime::block_on` there deadlocks the worker (the
    // outer runtime never polls). Run the bridge on a scoped thread instead —
    // the same pattern `emit_snapshot_blob` uses.
    Some(oxide_fw_bundler::TransformHook::new(move |id, source| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let id = id.to_string();
        let source = source.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> Result<oxide_fw_bundler::TransformOutput, oxide_fw_diagnostics::OxideError> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            OxideError(Box::new(
                                oxide_fw_diagnostics::Diagnostic::error(
                                    "PLG-BRIDGE",
                                    format!("bridge runtime: {e}"),
                                ),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        let mut code = source;
                        // V2-C06: the chain's accumulated map — each
                        // plugin's optional `map` field (a V3 JSON string)
                        // composes into this; a mapless plugin is
                        // position-preserving and keeps it intact.
                        let mut map: Option<String> = None;
                        for path in &plugins {
                            if need_load {
                                let load = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, load).await;
                            }
                            let call = oxide_fw_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "transform",
                                    "args": { "code": code, "id": id },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:transform"),
                                route: None,
                            };
                            let entry = oxide_fw_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "transform".into(),
                            };
                            let mut stream = pool.call(entry, call).await.map_err(|e| {
                                OxideError(Box::new(
                                    oxide_fw_diagnostics::Diagnostic::error(
                                        "PLG-CALL",
                                        format!("plugin transform failed: {}", e.0.message),
                                    ),
                                ))
                            })?;
                            let bytes = stream.collect().await.map_err(|e| {
                                OxideError(Box::new(
                                    oxide_fw_diagnostics::Diagnostic::error(
                                        "PLG-CALL",
                                        format!("plugin transform read failed: {}", e.0.message),
                                    ),
                                ))
                            })?;
                            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                                if let Some(s) = hook_result_str(&v, "code") {
                                    code = s;
                                }
                                // V2-C06: the plugin may carry a map beside
                                // the code (the protocol's `map` field).
                                fold_plugin_map(&mut map, extract_hook_map(&v));
                            }
                        }
                        Ok(oxide_fw_bundler::TransformOutput { code, map })
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(OxideError(Box::new(
                        oxide_fw_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "plugin bridge thread panicked",
                        ),
                    )))
                })
        })
    }))
}

// ─── SPEC-15.1 / §15.2: JS plugin chain ──────────────────────────────────
//
// A JS plugin is a Node ESM module exporting hook functions by name
// (`transform`, `resolveId`, `load`, …). The worker (`dispatchPluginCall`
// in `oxide-worker.mjs`) is a generic dispatcher — it takes `{ path, hook,
// args }` and calls `mod[hook](ctx, args)`. What lives HERE is the Rust
// side: load N plugin modules, validate their hook declarations at load
// time (SPEC-15.2 "fail at load"), and call a hook across the chain in
// the order the config declares them.
//
// This is the infrastructure §15-B hooks the bundler and prerender
// pipeline against; the wiring itself (BundleOptions.resolve_hook etc.)
// lands in a follow-up commit.
pub(crate) fn build_transform_hook(
    root: &Path,
    cfg: &OxideConfig,
) -> Option<oxide_fw_bundler::TransformHook> {
    for p in &cfg.plugins {
        let kind = p.get("kind").and_then(|v| v.as_str());
        if kind != Some("wasm") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            eprintln!("warn: wasm plugin entry has no `path`; skipping");
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, plugins_allow_absolute(cfg)) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&abs) else {
            eprintln!("warn: wasm plugin {} not readable; skipping", abs.display());
            continue;
        };

        let manifest = oxide_fw_plugin::WasmPluginManifest {
            name: p
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("wasm-plugin")
                .to_string(),
            wit_world: p
                .get("witWorld")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            capabilities: p
                .get("capabilities")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            hooks: p
                .get("hooks")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| match s {
                            "transform" => Some(oxide_fw_plugin::Hook::Transform),
                            "load" => Some(oxide_fw_plugin::Hook::Load),
                            "resolveId" | "resolve_id" => Some(oxide_fw_plugin::Hook::ResolveId),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };

        let plugin = match oxide_fw_plugin::WasmPlugin::load(&bytes, manifest, Some(&abs)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warn: wasm plugin {} failed to load: {} {}; skipping",
                    abs.display(),
                    e.0.code,
                    e.0.message
                );
                continue;
            }
        };
        // Only a plugin that declares *and* exports `transform` gets a hook.
        if !plugin
            .available_hooks()
            .contains(&oxide_fw_plugin::Hook::Transform)
        {
            continue;
        }
        let plugin = std::sync::Mutex::new(plugin);
        return Some(oxide_fw_bundler::TransformHook::new_code(
            move |_id, source| {
                // `call_string_hook` takes `&mut self` (the store is stateful);
                // a `Mutex` makes the closure `Sync`. Plugin transforms are not
                // hot enough for the lock to matter.
                let mut guard = plugin.lock().unwrap_or_else(|p| p.into_inner());
                guard.call_string_hook(&oxide_fw_plugin::Hook::Transform, source)
            },
        ));
    }
    None
}

pub(crate) fn load_client_assets(rootp: &Path) -> Option<oxide_fw_server::StaticAssets> {
    let dist = rootp.join("dist");
    let public = rootp.join("public");
    if public.is_dir() {
        // SEC-STATIC: a failed public/ copy used to be silently ignored
        // (`let _ =`), which shipped a site without favicon/robots and made the
        // failure invisible. Warn loudly, but do not fail the boot.
        if let Err(e) = copy_tree(&public, &dist) {
            tracing::warn!("copying public/ into dist/ failed: {e}");
        }
    }
    if !dist.is_dir() {
        return None;
    }
    // SEC-STATIC (P0-1): scan dist/ EXCLUDING the `server` directory. The
    // previous unfiltered scan indexed `dist/server/ssr/index.js`,
    // `dist/server/server.manifest.json` and `dist/server/l0-seed.json`, which
    // made the whole SSR bundle downloadable from the production port
    // (`curl /server/ssr/index.js`). `StaticAssets::serve` additionally
    // hard-denies those paths for defense in depth.
    oxide_fw_server::StaticAssets::scan_filtered(dist, &["server"]).ok()
}

/// Collect every candidate plugin entry that could export `hook_name`.
/// Reuses `confine_plugin_path` for the kind:js plugin paths (same rule the
/// transform path applies) so a path escape is caught at collect time.
pub(crate) fn collect_plugin_paths_for_hook(
    root: &Path,
    cfg: &OxideConfig,
    hook_name: &str,
) -> Vec<String> {
    let mut plugins: Vec<String> = Vec::new();
    let allow_abs = plugins_allow_absolute(cfg);
    for p in &cfg.plugins {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, allow_abs) else {
            continue;
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not found; skipping", abs.display());
            continue;
        }
        plugins.push(abs.to_string_lossy().to_string());
    }
    for m in crate::modules_loader::load_quiet(root, cfg) {
        if !m.hooks.iter().any(|h| h == hook_name) {
            continue;
        }
        let Some(ref entry) = m.entry else {
            eprintln!(
                "warn: module `{}` declares `{hook_name}` but has no entry file; skipping",
                m.name,
            );
            continue;
        };
        if !entry.is_file() {
            eprintln!(
                "warn: module `{}` entry {} is missing; skipping {hook_name}",
                m.name,
                entry.display(),
            );
            continue;
        }
        plugins.push(entry.to_string_lossy().to_string());
    }
    plugins
}

/// SPEC-15.2: build the `resolveId` hook. Chaining is Vite-style: the FIRST
/// plugin that returns a non-null `id` wins; a `{id: null}` passes to the
/// next plugin and finally to the bundler's default resolver.
pub(crate) fn build_js_plugin_resolve_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::ResolveHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "resolveId");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_fw_bundler::ResolveHook::new(
        move |specifier, from| {
            let pool = std::sync::Arc::clone(&pool);
            let loaded = std::sync::Arc::clone(&loaded);
            let plugins = plugins.clone();
            let specifier = specifier.to_string();
            let from = from.to_string();
            std::thread::scope(|scope| {
                scope
                    .spawn(|| -> oxide_fw_diagnostics::Result<Option<PathBuf>> {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                                    "PLG-BRIDGE",
                                    format!("bridge runtime: {e}"),
                                )))
                            })?;
                        rt.block_on(async move {
                            let need_load = {
                                let mut g = loaded.lock().unwrap();
                                if *g {
                                    false
                                } else {
                                    *g = true;
                                    true
                                }
                            };
                            for path in &plugins {
                                if need_load {
                                    let load = oxide_fw_jshost::CallInput {
                                        payload: serde_json::json!({ "path": path }),
                                        headers: Default::default(),
                                        has_body: false,
                                        request_id: format!("plugin:load:{path}"),
                                        route: None,
                                    };
                                    let entry = oxide_fw_jshost::EntryRef {
                                        kind: "plugin:load".into(),
                                        module: path.clone(),
                                        export: "default".into(),
                                    };
                                    let _ = pool.call(entry, load).await;
                                }
                                let call = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({
                                        "path": path,
                                        "hook": "resolveId",
                                        "args": { "specifier": specifier, "from": from },
                                    }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:call:{path}:resolveId"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:call".into(),
                                    module: path.clone(),
                                    export: "resolveId".into(),
                                };
                                // A plugin that does not export resolveId (or one
                                // whose call errors) passes to the next plugin.
                                let Ok(mut stream) = pool.call(entry, call).await else {
                                    continue;
                                };
                                let Ok(bytes) = stream.collect().await else {
                                    continue;
                                };
                                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                                else {
                                    continue;
                                };
                                if let Some(id_str) = hook_result_str(&v, "id") {
                                    return Ok(Some(PathBuf::from(id_str)));
                                }
                            }
                            Ok(None)
                        })
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(OxideError(Box::new(
                            oxide_fw_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                "plugin bridge thread panicked",
                            ),
                        )))
                    })
            })
        },
    ))
}

/// SPEC-15.2: build the `load` hook. Chaining: the FIRST plugin that
/// returns a non-null `code` overrides the disk read; `{code: null}` passes
/// to the next plugin and finally to disk.
pub(crate) fn build_js_plugin_load_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::LoadHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "load");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_fw_bundler::LoadHook::new(move |id| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let id = id.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_fw_diagnostics::Result<Option<String>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            )))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        for path in &plugins {
                            if need_load {
                                let load = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, load).await;
                            }
                            let call = oxide_fw_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "load",
                                    "args": { "id": id },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:load"),
                                route: None,
                            };
                            let entry = oxide_fw_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "load".into(),
                            };
                            let Ok(mut stream) = pool.call(entry, call).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(code) = hook_result_str(&v, "code") {
                                return Ok(Some(code));
                            }
                        }
                        Ok(None)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(OxideError(Box::new(
                        oxide_fw_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "plugin bridge thread panicked",
                        ),
                    )))
                })
        })
    }))
}

pub(crate) fn collect_plugin_options(
    root: &Path,
    cfg: &OxideConfig,
) -> Vec<(String, blake3::Hash, PathBuf)> {
    let mut out = Vec::new();
    for (idx, p) in cfg.plugins.iter().enumerate() {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
        };
        // The name is what the plugin object calls itself, or an
        // index-based fallback. A name carries no semantics beyond making
        // the diagnostic readable.
        let name = p
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("plugin#{idx}"));
        // Hash the whole config entry — every field the author put in the
        // object is an option the plugin received.
        let canonical = serde_json::to_vec(p).unwrap_or_default();
        out.push((name, blake3::hash(&canonical), abs));
    }
    out
}

/// §15-B: build the `renderChunk` hook. Chaining: each plugin's
/// `renderChunk(file, source)` receives the previous plugin's output; the
/// final string is what the bundler writes.
pub(crate) fn build_js_plugin_render_chunk_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::RenderChunkHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "renderChunk");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_fw_bundler::RenderChunkHook::new(
        move |file, source| {
            let pool = std::sync::Arc::clone(&pool);
            let loaded = std::sync::Arc::clone(&loaded);
            let plugins = plugins.clone();
            let file = file.to_string();
            let source = source.to_string();
            std::thread::scope(|scope| {
                scope
                    .spawn(|| -> oxide_fw_diagnostics::Result<Option<String>> {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                                    "PLG-BRIDGE",
                                    format!("bridge runtime: {e}"),
                                )))
                            })?;
                        rt.block_on(async move {
                            let need_load = {
                                let mut g = loaded.lock().unwrap();
                                if *g {
                                    false
                                } else {
                                    *g = true;
                                    true
                                }
                            };
                            let mut code = source;
                            let mut changed = false;
                            for path in &plugins {
                                if need_load {
                                    let req = oxide_fw_jshost::CallInput {
                                        payload: serde_json::json!({ "path": path }),
                                        headers: Default::default(),
                                        has_body: false,
                                        request_id: format!("plugin:load:{path}"),
                                        route: None,
                                    };
                                    let entry = oxide_fw_jshost::EntryRef {
                                        kind: "plugin:load".into(),
                                        module: path.clone(),
                                        export: "default".into(),
                                    };
                                    let _ = pool.call(entry, req).await;
                                }
                                let req = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({
                                        "path": path,
                                        "hook": "renderChunk",
                                        "args": { "file": file, "code": code },
                                    }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:call:{path}:renderChunk"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:call".into(),
                                    module: path.clone(),
                                    export: "renderChunk".into(),
                                };
                                let Ok(mut stream) = pool.call(entry, req).await else {
                                    continue;
                                };
                                let Ok(bytes) = stream.collect().await else {
                                    continue;
                                };
                                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                                else {
                                    continue;
                                };
                                if let Some(s) = hook_result_str(&v, "code") {
                                    code = s;
                                    changed = true;
                                }
                            }
                            Ok(if changed { Some(code) } else { None })
                        })
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(OxideError(Box::new(
                            oxide_fw_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                "renderChunk bridge thread panicked",
                            ),
                        )))
                    })
            })
        },
    ))
}

/// §15-B: build the `generateBundle` hook. Chaining: the LAST plugin that
/// returns a non-null file list wins (Vite convention).
pub(crate) fn build_js_plugin_generate_bundle_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::GenerateBundleHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "generateBundle");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_fw_bundler::GenerateBundleHook::new(move |files| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let files: Vec<String> = files.to_vec();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_fw_diagnostics::Result<Option<Vec<String>>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            )))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        let mut current: Option<Vec<String>> = None;
                        for path in &plugins {
                            if need_load {
                                let req = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, req).await;
                            }
                            let list = current.as_deref().unwrap_or(&files);
                            let req = oxide_fw_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "generateBundle",
                                    "args": { "files": list },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:generateBundle"),
                                route: None,
                            };
                            let entry = oxide_fw_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "generateBundle".into(),
                            };
                            let Ok(mut stream) = pool.call(entry, req).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(files) = hook_result_str_array(&v, "files") {
                                current = Some(files);
                            }
                        }
                        Ok(current)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(OxideError(Box::new(
                        oxide_fw_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "generateBundle bridge thread panicked",
                        ),
                    )))
                })
        })
    }))
}

/// §15-B: build the `transformHtml` hook. Chaining: each plugin's
/// `transformHtml(route_id, html)` receives the previous plugin's output.
pub(crate) fn build_js_plugin_transform_html_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<oxide_fw_bundler::TransformHtmlHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "transformHtml");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_fw_bundler::TransformHtmlHook::new(
        move |route_id, html| {
            let pool = std::sync::Arc::clone(&pool);
            let loaded = std::sync::Arc::clone(&loaded);
            let plugins = plugins.clone();
            let route_id = route_id.to_string();
            let html = html.to_string();
            std::thread::scope(|scope| {
                scope
                    .spawn(|| -> oxide_fw_diagnostics::Result<Option<String>> {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                                    "PLG-BRIDGE",
                                    format!("bridge runtime: {e}"),
                                )))
                            })?;
                        rt.block_on(async move {
                            let need_load = {
                                let mut g = loaded.lock().unwrap();
                                if *g {
                                    false
                                } else {
                                    *g = true;
                                    true
                                }
                            };
                            let mut doc = html;
                            let mut changed = false;
                            for path in &plugins {
                                if need_load {
                                    let req = oxide_fw_jshost::CallInput {
                                        payload: serde_json::json!({ "path": path }),
                                        headers: Default::default(),
                                        has_body: false,
                                        request_id: format!("plugin:load:{path}"),
                                        route: None,
                                    };
                                    let entry = oxide_fw_jshost::EntryRef {
                                        kind: "plugin:load".into(),
                                        module: path.clone(),
                                        export: "default".into(),
                                    };
                                    let _ = pool.call(entry, req).await;
                                }
                                let req = oxide_fw_jshost::CallInput {
                                    payload: serde_json::json!({
                                        "path": path,
                                        "hook": "transformHtml",
                                        "args": { "route": route_id, "html": doc },
                                    }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:call:{path}:transformHtml"),
                                    route: None,
                                };
                                let entry = oxide_fw_jshost::EntryRef {
                                    kind: "plugin:call".into(),
                                    module: path.clone(),
                                    export: "transformHtml".into(),
                                };
                                let Ok(mut stream) = pool.call(entry, req).await else {
                                    continue;
                                };
                                let Ok(bytes) = stream.collect().await else {
                                    continue;
                                };
                                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                                else {
                                    continue;
                                };
                                if let Some(s) = hook_result_str(&v, "html") {
                                    doc = s;
                                    changed = true;
                                }
                            }
                            Ok(if changed { Some(doc) } else { None })
                        })
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(OxideError(Box::new(
                            oxide_fw_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                "transformHtml bridge thread panicked",
                            ),
                        )))
                    })
            })
        },
    ))
}

/// Extract `field` from a hook result. Accepts a bare string or
/// `{ "<field>": "…" }`.
pub(crate) fn hook_result_str(v: &serde_json::Value, field: &str) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    v.get(field).and_then(|x| x.as_str()).map(str::to_string)
}

/// V2-C06: extract the transform hook's optional `map` field (a V3 JSON
/// string). Only the object form `{ code, map }` carries one — a bare-string
/// result is code-only by contract — so unlike [`hook_result_str`] there is
/// no bare-value fallback.
pub(crate) fn extract_hook_map(v: &serde_json::Value) -> Option<String> {
    v.get("map").and_then(|x| x.as_str()).map(str::to_string)
}

/// Extract `field` from a hook result as a string array. Accepts a bare
/// array of strings or `{ "<field>": [...] }`.
pub(crate) fn hook_result_str_array(v: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    let arr = match v {
        serde_json::Value::Array(a) => a,
        _ => v.get(field)?.as_array()?,
    };
    Some(
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
    )
}

/// Call `hook_name` across every plugin/module that declares it. Returns the
/// per-plugin results in declaration order. An empty vec means no plugin
/// declared it — the caller skips. A plugin whose call fails is skipped, not
/// fatal: a lifecycle notification must not fail a build.
///
/// Loads each candidate first (`plugin:load` is idempotent on the worker —
/// it replaces the cached module). A lifecycle hook is called once per build
/// per plugin, so the load cost is negligible.
pub(crate) async fn call_js_plugin_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: &std::sync::Arc<dyn oxide_fw_jshost::Pool>,
    hook_name: &str,
    args: serde_json::Value,
) -> Vec<serde_json::Value> {
    let plugins = collect_plugin_paths_for_hook(root, cfg, hook_name);
    if plugins.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    for path in &plugins {
        let load = oxide_fw_jshost::CallInput {
            payload: serde_json::json!({ "path": path }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("plugin:load:{path}"),
            route: None,
        };
        let load_entry = oxide_fw_jshost::EntryRef {
            kind: "plugin:load".into(),
            module: path.clone(),
            export: "default".into(),
        };
        let _ = pool.call(load_entry, load).await;

        let req = oxide_fw_jshost::CallInput {
            payload: serde_json::json!({
                "path": path,
                "hook": hook_name,
                "args": args,
            }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("plugin:call:{path}:{hook_name}"),
            route: None,
        };
        let entry = oxide_fw_jshost::EntryRef {
            kind: "plugin:call".into(),
            module: path.clone(),
            export: hook_name.to_string(),
        };
        let Ok(mut stream) = pool.call(entry, req).await else {
            continue;
        };
        let Ok(bytes) = stream.collect().await else {
            continue;
        };
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            results.push(v);
        }
    }
    results
}

/// Fire a lifecycle hook from an async `cmd_*`. Async (not `block_on`):
/// every caller is already inside a runtime, and blocking on a future from
/// within a runtime panics.
pub(crate) async fn fire_lifecycle_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    hook_name: &str,
    args: serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(pool) = pool else {
        return Vec::new();
    };
    call_js_plugin_hook(root, cfg, pool, hook_name, args).await
}

/// True when any configured `kind:js` plugin (or module entry) exports
/// `hook_name`. Used to skip building a tooling pool for a hook nothing
/// declares — the pool costs one Node child.
pub(crate) fn plugin_declares_any(root: &Path, cfg: &OxideConfig, hook_names: &[&str]) -> bool {
    hook_names
        .iter()
        .any(|h| !collect_plugin_paths_for_hook(root, cfg, h).is_empty())
}

/// Merge a `config` hook's returned patch into the live config.
///
/// The patch is a partial JSON object whose keys match the config's own.
/// Only the fields a plugin would realistically set are honored — a
/// wholesale re-serialize would lose every field this CLI does not model.
/// An unknown key is reported and skipped (a plugin targeting a newer config
/// field should not fail a build).
pub(crate) fn apply_config_patch(
    cfg: &mut OxideConfig,
    patch: &serde_json::Value,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let Some(obj) = patch.as_object() else {
        return Ok(());
    };
    for (key, value) in obj {
        match key.as_str() {
            "adapter" => {
                if let Some(s) = value.as_str() {
                    cfg.adapter = s.to_string();
                }
            }
            "siteUrl" | "site_url" => {
                if let Some(s) = value.as_str() {
                    cfg.site_url = Some(s.to_string());
                }
            }
            "routes" => {
                if let Some(dir) = value.get("dir").and_then(|d| d.as_str()) {
                    cfg.routes.dir = dir.to_string();
                }
            }
            "build" => {
                if let Some(t) = value.get("target").and_then(|v| v.as_str()) {
                    cfg.build.target = t.to_string();
                }
            }
            other => {
                outln!(
                    w,
                    "warn: plugin config patch key `{other}` is not modeled by this CLI; ignored"
                );
            }
        }
    }
    Ok(())
}

/// §15.2: `renderHead` — a plugin contributes to the neutral head model.
/// Each result is `{ meta?: [...], link?: [...], style?: [...], script?: [...] }`,
/// merged into the model the adapter receives. Called once per prerendered
/// route (the hook sees the route id), not per request.
pub(crate) async fn plugin_render_head(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    route_id: &str,
) -> serde_json::Value {
    let mut merged = serde_json::json!({
        "meta": [], "link": [], "style": [], "script": []
    });
    let Some(pool) = pool else { return merged };
    if collect_plugin_paths_for_hook(root, cfg, "renderHead").is_empty() {
        return merged;
    }
    let results = call_js_plugin_hook(
        root,
        cfg,
        pool,
        "renderHead",
        serde_json::json!({ "route": route_id }),
    )
    .await;
    let Some(obj) = merged.as_object_mut() else {
        return merged;
    };
    for r in results {
        for field in ["meta", "link", "style", "script"] {
            if let Some(arr) = r.get(field).and_then(|v| v.as_array())
                && let Some(slot) = obj.get_mut(field).and_then(|v| v.as_array_mut())
            {
                slot.extend(arr.iter().cloned());
            }
        }
    }
    merged
}

/// §15.2: `cacheHandler` — a plugin *selects* the cache backend.
///
/// A plugin cannot implement `oxide_fw_cache::CacheHandler` itself: the trait is
/// async Rust over a Rust `Entry` (body bytes, policy, etag), and every
/// operation would cross an RPC boundary. The hook's real value is choosing
/// among the drivers the CLI already builds — a deploy plugin knows whether
/// its platform wants redis, s3, an HTTP cache, or memory.
///
/// Each plugin returns `{ kind: "memory" | "disk" | "redis" | "s3" | "http",
/// url?, bucket? }`. The last plugin to return a valid kind wins. An unknown
/// kind, or a missing url/bucket, is reported and the config's choice stands.
pub(crate) async fn resolve_cache_handler_via_plugin(
    root: &Path,
    cfg: &OxideConfig,
    w: Writer<'_>,
) -> anyhow::Result<Option<oxide_fw_config::CacheHandlerConfig>> {
    if collect_plugin_paths_for_hook(root, cfg, "cacheHandler").is_empty() {
        return Ok(None);
    }
    // The hook runs on the tooling pool — which is built without a cache
    // handler, since choosing it is the whole point.
    use oxide_fw_jshost::JsHost as _;
    let host = oxide_fw_jshost::NodeHost::new(oxide_fw_jshost::NodeHost::worker_script_default());
    let Some(pool) = host
        .tooling_pool(oxide_fw_jshost::PoolConfig::default())
        .ok()
    else {
        return Ok(None);
    };
    let results = call_js_plugin_hook(
        root,
        cfg,
        &pool,
        "cacheHandler",
        serde_json::json!({ "current": format!("{:?}", cfg.cache.handler) }),
    )
    .await;
    let mut chosen: Option<oxide_fw_config::CacheHandlerConfig> = None;
    for r in results {
        let Some(kind) = r.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        let candidate = match kind {
            "memory" => Some(oxide_fw_config::CacheHandlerConfig::Memory),
            "disk" => Some(oxide_fw_config::CacheHandlerConfig::Disk),
            "redis" => r.get("url").and_then(|u| u.as_str()).map(|url| {
                oxide_fw_config::CacheHandlerConfig::Redis {
                    url: url.to_string(),
                }
            }),
            "s3" => r.get("bucket").and_then(|b| b.as_str()).map(|bucket| {
                oxide_fw_config::CacheHandlerConfig::S3 {
                    bucket: bucket.to_string(),
                }
            }),
            "http" => r.get("url").and_then(|u| u.as_str()).map(|url| {
                oxide_fw_config::CacheHandlerConfig::Http {
                    url: url.to_string(),
                }
            }),
            other => {
                outln!(
                    w,
                    "warn: cacheHandler plugin returned unknown kind `{other}`; ignored"
                );
                None
            }
        };
        if let Some(c) = candidate {
            chosen = Some(c);
        } else if matches!(kind, "redis" | "s3" | "http") {
            outln!(
                w,
                "warn: cacheHandler kind `{kind}` is missing its url/bucket; ignored"
            );
        }
    }
    Ok(chosen)
}

/// Render a plugin's neutral head model (`renderHead`'s return value) into
/// `<head>` markup, for the placeholder path where no framework renderer
/// runs. The full model a real adapter merges is richer (title templates,
/// JSON-LD); this covers the four fields a plugin contributes — meta, link,
/// style, script — which is what the placeholder document can carry.
pub(crate) fn neutral_head_to_html(head: &serde_json::Value) -> String {
    let mut out = String::new();
    let Some(obj) = head.as_object() else {
        return out;
    };
    // `title` is a scalar, not an array — a route's `meta` sets it and it
    // belongs first in the head.
    if let Some(t) = obj.get("title").and_then(|v| v.as_str()) {
        out.push_str(&format!("<title>{}</title>", html_escape(t)));
    }
    for field in ["meta", "link", "style", "script"] {
        let Some(arr) = obj.get(field).and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in arr {
            let Some(attrs) = entry.as_object() else {
                // A bare string: treat as the element's text content.
                if let Some(text) = entry.as_str() {
                    let tag = if field == "style" { "style" } else { "script" };
                    out.push_str(&format!("<{tag}>{}</{tag}>", html_escape(text)));
                }
                continue;
            };
            let tag = match field {
                "meta" => "meta",
                "link" => "link",
                "style" => "style",
                "script" => "script",
                _ => continue,
            };
            // A `<style>` / `<script>` entry may carry `content` as body
            // text. For any other tag (`meta`, `link`) `content` is a normal
            // attribute — `<meta name="description" content="...">` is the
            // common case, and skipping it there emitted an attribute-less
            // tag.
            let body = if matches!(tag, "style" | "script") {
                attrs.get("content").and_then(|c| c.as_str())
            } else {
                None
            };
            let mut tag_out = format!("<{tag}");
            let mut keys: Vec<&String> = attrs.keys().collect();
            keys.sort();
            for k in keys {
                if k == "content" && body.is_some() {
                    continue;
                }
                if let Some(v) = attrs.get(k).and_then(|v| v.as_str()) {
                    tag_out.push_str(&format!(" {}=\"{}\"", k, html_escape(v)));
                }
            }
            match body {
                Some(b) if matches!(tag, "style" | "script") => {
                    tag_out.push_str(&format!(">{}</{tag}>", html_escape(b)));
                }
                _ => tag_out.push('>'),
            }
            out.push_str(&tag_out);
        }
    }
    // §16.3: JSON-LD is a structured value, not a `script.content` string.
    // Emit it as `<script type="application/ld+json">` so a `defineMeta`
    // that returns structured data reaches the head.
    if let Some(arr) = obj.get("jsonLd").and_then(|v| v.as_array()) {
        for entry in arr {
            let body = entry
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| entry.to_string());
            out.push_str(&format!(
                "<script type=\"application/ld+json\">{}</script>",
                html_escape(&body)
            ));
        }
    }
    out
}

/// Minimal HTML-attribute/text escape for the placeholder path.
pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Normalize a route's flat `meta` export into the neutral head model.
///
/// `defineMeta` historically returns a flat object (`{ title, description,
/// openGraph, canonical, jsonLd }`). The neutral model is structured
/// (`{ title, meta: [...], link: [...], jsonLd: [...] }`). Projects that
/// already return the structured shape pass through unchanged; a flat object
/// is lifted field-by-field so richer fields are not silently dropped.
pub(crate) fn normalize_route_meta(meta: serde_json::Value) -> serde_json::Value {
    let Some(obj) = meta.as_object() else {
        return meta;
    };
    const NEUTRAL: [&str; 5] = ["meta", "link", "style", "script", "jsonLd"];
    if NEUTRAL.iter().any(|k| obj.contains_key(*k)) {
        return meta;
    }
    let mut out = serde_json::Map::new();
    let mut metas: Vec<serde_json::Value> = Vec::new();
    let mut links: Vec<serde_json::Value> = Vec::new();
    let mut json_ld: Vec<serde_json::Value> = Vec::new();
    let stringify = |v: &serde_json::Value| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    };
    for (k, v) in obj {
        match k.as_str() {
            "title" | "titleTemplate" | "base" => {
                out.insert(k.clone(), v.clone());
            }
            "canonical" | "canonicalUrl" => {
                links.push(serde_json::json!({"rel": "canonical", "href": stringify(v)}));
            }
            "openGraph" | "og" => {
                if let Some(o) = v.as_object() {
                    for (ok, ov) in o {
                        metas.push(serde_json::json!({
                            "property": format!("og:{ok}"),
                            "content": stringify(ov),
                        }));
                    }
                }
            }
            "twitter" => {
                if let Some(o) = v.as_object() {
                    for (tk, tv) in o {
                        metas.push(serde_json::json!({
                            "name": format!("twitter:{tk}"),
                            "content": stringify(tv),
                        }));
                    }
                }
            }
            "jsonLd" | "json_ld" => match v {
                serde_json::Value::Array(a) => json_ld.extend(a.iter().cloned()),
                _ => json_ld.push(v.clone()),
            },
            _ => {
                metas.push(serde_json::json!({"name": k, "content": stringify(v)}));
            }
        }
    }
    out.insert("meta".into(), serde_json::Value::Array(metas));
    if !links.is_empty() {
        out.insert("link".into(), serde_json::Value::Array(links));
    }
    if !json_ld.is_empty() {
        out.insert("jsonLd".into(), serde_json::Value::Array(json_ld));
    }
    serde_json::Value::Object(out)
}

/// Merge one neutral head model into another.
///
/// The shape is `{meta: [...], link: [...], style: [...], script: [...],
/// title?: "..."}`. Arrays concatenate with `inner` last, so a route's entry
/// lands after a plugin's and a consumer reading the last match sees the page
/// author's value. `title` is inner-wins.
pub(crate) fn merge_neutral_head(
    outer: serde_json::Value,
    inner: serde_json::Value,
) -> serde_json::Value {
    let mut out = match outer {
        serde_json::Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    let Some(inner_obj) = inner.as_object() else {
        return serde_json::Value::Object(out);
    };
    for field in ["meta", "link", "style", "script", "jsonLd"] {
        match (out.get_mut(field), inner_obj.get(field)) {
            (Some(serde_json::Value::Array(a)), Some(serde_json::Value::Array(b))) => {
                a.extend(b.iter().cloned());
            }
            (None, Some(v)) => {
                out.insert(field.to_string(), v.clone());
            }
            _ => {}
        }
    }
    for scalar in ["title", "titleTemplate", "base"] {
        if let Some(t) = inner_obj.get(scalar) {
            out.insert(scalar.to_string(), t.clone());
        }
    }
    serde_json::Value::Object(out)
}
