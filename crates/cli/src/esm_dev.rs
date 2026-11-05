//! Native ESM dev server (Phase 1 of the HMR design in `docs/ideas/hmr.md`).
//!
//! When `dev.esm = true` in `oxide.config.ts`, `oxide dev` skips the client
//! bundler and serves source files as transformed ESM. Four URL families are
//! served by [`handle_esm_request`]:
//!
//! - `/src/**/*.tsx` — raw source, lowered to ESM by Oxc, prefixed with a
//!   small preamble that installs `import.meta.hot` from
//!   `globalThis.__oxide_hmr__`.
//! - `/@prebundle/<name>.js` — prebundled deps (React etc.; Phase 1b) plus
//!   the framework dist files copied from `packages/core/dist/` (Phase 1a.5).
//! - `/@boot/boot.mjs` — the boot entry that imports `@oxide-fw/core/client/hot`
//!   first (side-effect installs the HMR runtime) then each route component.
//! - `/@esm-error/<encoded>` — Vite's pattern for a failed dynamic import.
//!
//! ## Import rewriting
//!
//! No importmap: the browser's native ESM resolver cannot see `node_modules`
//! specifiers. Instead, every served JavaScript file has its bare specifiers
//! (`react`, `react-dom/client`, `@oxide-fw/core/client/...`) rewritten to
//! absolute `/@prebundle/...` URLs before it leaves the dev server. Relative
//! imports (`./foo.js`) resolve naturally and are left alone.
//!
//! The rewriting uses [`oxide_fw_compiler::scan_import_spans`], which tokenizes
//! the source (skipping strings, templates, comments), then splices the
//! specifier strings from the back to keep byte offsets stable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxide_fw_compiler::{inject_hmr_boundary, lower_to_js_with_refresh};
use oxide_fw_server::{OxideRequest, OxideResponse};

/// Directory (project-relative) where prebundled deps and copied framework
/// dist files live. URL-mapped to `/@prebundle/`.
pub const PREBUNDLE_DIR: &str = ".oxide/esm-prebundle";

/// §1.4: pre-compute the JS module for every `.module.css` in `src/`.
///
/// The SSR worker's Node loader cannot itself run lightningcss (it's a
/// pure-Node loader with no Rust callback). The CLIENT `/@css/` path
/// transforms on demand via `serve_css_module` (Rust-side lightningcss).
/// Both MUST produce identical mangled class names for React hydration to
/// reconcile — so the transform is computed once, by the SAME Rust code
/// (`LightningCssBackend`), and written to disk where the loader reads it.
///
/// Writes `.oxide/css-modules/<rel>.mjs` where `<rel>` is the src-relative
/// path with `/` replaced by `__` (filesystem-safe, reversible by the
/// loader). Content is `export default {"local":"mangled",...};` — exactly
/// what `serve_css_module` exports over the wire.
///
/// Called:
///   - At dev boot (cmd_dev), so the first SSR request works.
///   - On a `.module.css` change (watcher's StyleSwap branch), so SSR sees
///     the new map on the next request while the CLIENT injects the new
///     style via the existing /@css/ reload.
///
/// Best-effort: a `.module.css` that fails to transform is skipped with a
/// warning; the loader falls back to `export default ""` for that file
/// (which produces an unstyled SSR element, matching prior behaviour for
/// that file — never a hard failure of the whole page).
pub fn precompute_css_modules(root: &Path) -> anyhow::Result<()> {
    let src = root.join("src");
    if !src.is_dir() {
        return Ok(());
    }
    let out = root.join(".oxide").join("css-modules");
    std::fs::create_dir_all(&out)?;
    use oxide_fw_assets::{CssBackend, CssInput, CssKind};
    let backend = oxide_fw_assets::LightningCssBackend;
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let fname = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
            let is_mod = fname.ends_with(".module.css")
                || fname.ends_with(".module.scss")
                || fname.ends_with(".module.sass");
            if !is_mod {
                continue;
            }
            let Ok(code) = std::fs::read_to_string(&p) else { continue };
            let rel_path = p
                .strip_prefix(&src)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| fname.to_string());
            let input = CssInput {
                code,
                // Same `file` the client `serve_css_module` passes —
                // REQUIRED for identical hash. `src/{rel_path}`.
                file: Some(format!("src/{rel_path}")),
                kind: CssKind::Module,
                minify: false,
                targets: Vec::new(),
            };
            match backend.transform(&input) {
                Ok(res) => {
                    let json = serde_json::to_string(&res.exports)
                        .unwrap_or_else(|_| "{}".to_string());
                    let js = format!("export default {json};\n");
                    let rel_enc = rel_path.replace('/', "__");
                    let dst = out.join(format!("{rel_enc}.mjs"));
                    if let Err(e) = std::fs::write(&dst, js) {
                        tracing::warn!(
                            path = %dst.display(),
                            error = %e,
                            "§1.4: could not write pre-computed CSS module"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %p.display(),
                        code = %e.0.code,
                        error = %e.0.message,
                        "§1.4: CSS modules transform failed; SSR will use empty default"
                    );
                }
            }
        }
    }
    Ok(())
}

/// The ESM manifest. Mirrors `dist/client/client.manifest.json`'s shape.
pub const MANIFEST_PATH: &str = ".oxide/esm-manifest.json";
/// The boot entry, URL-served at `/@boot/boot.mjs`.
pub const BOOT_ENTRY_PATH: &str = ".oxide/esm-boot.mjs";
/// The boot entry's URL (server-relative). Threaded to the injector.
pub const BOOT_SCRIPT_URL: &str = "/@boot/boot.mjs";
/// URL prefix for CSS modules imported from a route component. Vite-style:
/// `import './styles.css'` in a `.tsx` file becomes `import '/@css/<path>'`,
/// which returns a JS module that injects a `<style>` tag and self-accepts
/// for HMR. A CSS file whose content contains `@import "tailwindcss"` is
/// served as an empty stub — the actual CSS reaches the browser through the
/// SSR renderer's disk-scan `<link>` (see `inject_route_css`), not through
/// this handler.
pub const CSS_URL_PREFIX: &str = "/@css/";
/// URL prefix for imports of server-only modules (`*.server.ts`) from a
/// client component. Vite's equivalent is `vite:import-analysis`'s
/// server-reference rewrite; Next.js uses `server-reference-manifest`.
/// The specifier is rewritten at serve time to `/@server-fn/<path>`, and the
/// handler runs the same `transform_server_fn_stubs` pass the production
/// build uses (`Env::Client`) to produce `__oxide_call_fn` proxies.
///
/// Without this: `import { greet } from "./greeting.server.ts"` is served
/// verbatim. The browser fetches the file, the `.server.` guard returns 403
/// (correct), and the whole module graph halts — no hydration, no HMR, no
/// error on the page. That was the last regression.
pub const SERVER_FN_URL_PREFIX: &str = "/@server-fn/";

/// Everything the ESM request handler needs. Built once at dev startup,
/// shared through `Arc` with the dispatch closure.
pub struct EsmDevState {
    pub root: PathBuf,
    pub jsx_import_source: Option<String>,
    /// Bare specifier → `/@prebundle/...` URL. Used both to rewrite the
    /// imports of served files and to compose the boot entry.
    pub rewrite_map: Arc<BTreeMap<String, String>>,
    /// The configured framework (`react | vue | svelte | solid`). Selects
    /// which `@oxide-fw/*` package the boot entry wires as `clientAdapter`
    /// and which `/src/*` files need a Node tooling-pool compile (Vue SFC,
    /// Svelte, Solid tsx) instead of the native Oxc lowering.
    pub framework: String,
}

/// What [`handle_esm_request`] decided for a `/src/*` request.
pub enum EsmAction {
    /// Serve this response directly.
    Ready(OxideResponse),
    /// The module needs a Node tooling-pool compile before it can be served
    /// (`.vue`/`.svelte` SFCs, and `.tsx`/`.jsx` when the framework is
    /// Solid — babel-preset-solid's output is the only client-correct Solid
    /// transform this repo can run on every platform). The payload is the
    /// src-relative path, query-stripped.
    PoolModule(String),
}

/// True when `rel_path` (src-relative, extension included) must be compiled
/// through the Node tooling pool instead of the native Oxc lowering.
///
/// - `.vue` / `.svelte`: `@vue/compiler-sfc` / `svelte/compiler` are JS
///   packages (crates/compiler/src/framework.rs routes them through the
///   tooling pool by design — there is no native SFC parser).
/// - `.tsx`/`.jsx` on Solid: the Oxc lowering emits plain `jsx()` calls,
///   which for Solid must come from `@solidjs/web/jsx-runtime`. The
///   `babel-preset-solid` DOM transform in the pool produces the real
///   `template()/insert()/createComponent()` output the client runtime and
///   hydration expect, and — unlike the optional `solid-jsx-oxc` native
///   binding — babel is a pure-JS dependency available on every platform.
pub fn needs_pool_compile(state: &EsmDevState, rel_path: &str) -> bool {
    let lower = rel_path.to_ascii_lowercase();
    if lower.ends_with(".vue") || lower.ends_with(".svelte") {
        return true;
    }
    if state.framework == "solid"
        && (lower.ends_with(".tsx") || lower.ends_with(".jsx"))
    {
        return true;
    }
    false
}

/// The tooling-pool `EntryRef.kind` + the `generate` mode for a
/// pool-compiled client module.
fn pool_op_for(rel_path: &str) -> (&'static str, &'static str) {
    let lower = rel_path.to_ascii_lowercase();
    if lower.ends_with(".vue") {
        ("tooling:vue-compile", "dom")
    } else if lower.ends_with(".svelte") {
        ("tooling:svelte-compile", "dom")
    } else {
        ("tooling:solid-compile", "dom")
    }
}

/// Compile a `.vue`/`.svelte`/solid-`.tsx` file to a CLIENT ESM module via
/// the Node tooling pool and serve it with the same preamble + HMR boundary
/// + import rewriting [`serve_source_file`] applies.
///
/// Called from the dispatch closure (which can await the pool) whenever
/// [`needs_pool_compile`] matched. The source is read fresh per request —
/// an HMR re-import after an edit always sees the newest bytes, which is
/// what makes the watcher's single `ModuleUpdate` publish sufficient (no
/// server-side precompile + invalidation bookkeeping for client SFCs).
///
/// On a pool error the response is a JS module that throws — the browser
/// surfaces it through the HMR overlay (`HMR-IMPORT`) instead of silently
/// serving stale code.
pub async fn serve_pool_compiled_module(
    state: &EsmDevState,
    rel_path: &str,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> OxideResponse {
    // Refuse `.server.*` like the plain source path — a Solid page may not
    // import them, but the guard belongs to the URL family, not the
    // transform.
    if rel_path.contains(".server.") {
        return OxideResponse::new(403, "server module cannot be served to the client")
            .with_header("content-type", "text/plain; charset=utf-8")
            .with_header("cache-control", "no-cache");
    }
    let fail = |msg: String| -> OxideResponse {
        let body = format!(
            "// oxide dev: pool compile failed for src/{rel_path}\n\
             throw new Error({msg:?});\n"
        );
        OxideResponse::new(500, body)
            .with_header("content-type", "text/javascript; charset=utf-8")
            .with_header("cache-control", "no-cache")
    };
    let abs = state.root.join("src").join(rel_path);
    let src = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(_) => {
            return fail(format!("source not found: src/{rel_path}"));
        }
    };
    let Some(pool) = pool else {
        return fail("tooling pool unavailable — cannot compile SFC/Solid client module".into());
    };
    let (op_kind, generate) = pool_op_for(rel_path);
    let payload = serde_json::json!({
        "source": src,
        "filename": format!("src/{rel_path}"),
        "generate": generate,
        // The Solid fallback (babel-preset-solid) resolves @babel/core from
        // the PROJECT's node_modules — the src-relative filename cannot
        // locate it, so pass the root explicitly. Other ops ignore it.
        "root": state.root.to_string_lossy(),
    });
    let req = oxide_fw_jshost::CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: format!("esm-sfc:{rel_path}"),
        route: None,
    };
    let entry = oxide_fw_jshost::EntryRef {
        kind: op_kind.to_string(),
        module: String::new(),
        export: String::new(),
    };
    let mut stream = match pool.call(entry, req).await {
        Ok(s) => s,
        Err(e) => return fail(format!("{} {}", e.0.code, e.0.message)),
    };
    let bytes = match stream.collect().await {
        Ok(b) => b,
        Err(e) => return fail(format!("{} {}", e.0.code, e.0.message)),
    };
    let v: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return fail(format!("worker returned invalid JSON: {e}")),
    };
    if let Some(err) = v.get("__oxide_error").and_then(|x| x.as_str()) {
        return fail(err.to_string());
    }
    let js = match v.get("js").and_then(|x| x.as_str()) {
        Some(js) => js.to_string(),
        None => return fail("compile op returned no `js` field".into()),
    };
    // HMR boundary. The lang is deliberately `"ts"` (NOT the file's own
    // extension): the boundary picks the accept shape by lang, and for
    // these frameworks the correct shape is the generic self-accept — the
    // in-place update is driven by the boot runtime's re-render hook, not
    // by React Refresh. Passing the real extension would emit the
    // `__oxide_refresh(mod)` accept, which falls back to `location.reload()`
    // without a React Refresh runtime — a full reload on every Solid edit.
    let path_for_compiler = format!("src/{rel_path}");
    let with_hmr = inject_hmr_boundary(&js, &path_for_compiler, "ts").unwrap_or(js);
    let rewritten = rewrite_imports(
        &with_hmr,
        &state.rewrite_map,
        Some(rel_path),
        Some(&state.root.join("src")),
    );
    let preamble = hot_preamble(state, &format!("/src/{rel_path}"));
    let out = format!("{preamble}{rewritten}");
    OxideResponse::new(200, out)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

/// The per-module preamble: hot runtime import + `import.meta.hot` handle.
/// Shared by [`serve_source_file`] and [`serve_pool_compiled_module`].
fn hot_preamble(state: &EsmDevState, url: &str) -> String {
    let hot_url = state
        .rewrite_map
        .get("@oxide-fw/core/client/hot")
        .cloned()
        .unwrap_or_else(|| "@oxide-fw/core/client/hot".to_string());
    let url_literal = format!("{url:?}");
    format!(
        "import {hot_url:?};\n\
         if (!import.meta.hot && globalThis.__oxide_hmr__) {{\n\
           import.meta.hot = globalThis.__oxide_hmr__.createHotContext({url_literal});\n\
         }}\n"
    )
}

// --- Manifest ------------------------------------------------------------

/// Write `.oxide/esm-manifest.json`. Shape mirrors `client.manifest.json`:
/// `{ "routes": { "<id>": { "component": "/src/<path>" } }, "build_id": "dev-esm" }`.
pub fn build_esm_manifest(
    root: &Path,
    components: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let out = root.join(MANIFEST_PATH);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut routes = serde_json::Map::new();
    for (id, url) in components {
        let mut comp = serde_json::Map::new();
        comp.insert(
            "component".into(),
            serde_json::Value::String(url.clone()),
        );
        routes.insert(id.clone(), serde_json::Value::Object(comp));
    }
    let manifest = serde_json::json!({
        "routes": routes,
        "build_id": "dev-esm",
    });
    std::fs::write(&out, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

/// Write `.oxide/esm-boot.mjs`: the entry the browser loads. Import order
/// matters — the hot runtime must install **before** any component module is
/// evaluated (its `import.meta.hot` is read at module top-level).
pub fn generate_esm_boot_entry(
    root: &Path,
    components: &BTreeMap<String, String>,
    islands: &BTreeMap<String, String>,
    framework: &str,
) -> anyhow::Result<PathBuf> {
    let out = root.join(BOOT_ENTRY_PATH);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let exports = read_framework_exports(root).unwrap_or_default();
    let lookup: BTreeMap<&str, &str> = exports
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let hot_url = lookup
        .get("@oxide-fw/core/client/hot")
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "packages/core/package.json is missing the `./client/hot` export"
            )
        })?;

    let mut src = String::new();
    src.push_str("// Generated by `oxide dev` (native ESM mode). Do not edit.\n");
    src.push_str("import ");
    src.push_str(&format!("{hot_url:?}"));
    src.push_str(";\n");
    // The main client barrel is best-effort: it pulls in React via hydration
    // glue, which is only available once Phase 1b's prebundle lands. When it's
    // missing the HMR runtime still installs (from the hot.ts import above).
    if let Some(client_url) = lookup.get("@oxide-fw/core/client") {
        src.push_str("import ");
        src.push_str(&format!("{client_url:?}"));
        src.push_str(";\n");
    }
    for url in components.values() {
        src.push_str("import ");
        src.push_str(&format!("{url:?}"));
        src.push_str(";\n");
    }
    // Wire the CONFIGURED framework's client adapter. The previous hard-
    // coded `@oxide-fw/react` lookup meant a Vue/Solid/Svelte project booted
    // with `__adapter = undefined` — no `bootWithRouter`, no hydration, no
    // HMR WebSocket, no `adapter.update`: the page rendered SSR-only forever
    // and every module-update frame was dropped on the floor. Fall back to
    // whichever framework package actually ships exports, so a mis-detected
    // config degrades to a working page instead of a blank one.
    let framework_candidates: [&str; 4] = ["react", "vue", "solid", "svelte"];
    let adapter_specifier = (if framework_candidates.contains(&framework) {
        Some(format!("@oxide-fw/{framework}"))
    } else {
        None
    })
    .filter(|spec| lookup.contains_key(spec.as_str()))
    .or_else(|| {
        framework_candidates
            .iter()
            .map(|f| format!("@oxide-fw/{f}"))
            .find(|spec| lookup.contains_key(spec.as_str()))
    });
    if let Some(adapter_spec) = adapter_specifier {
        if let Some(adapter_url) = lookup.get(adapter_spec.as_str()) {
            src.push_str(&format!(
                "import {{ clientAdapter }} from {adapter_url:?};\n"
            ));
        }
    }
    let boot_fn_url = lookup
        .get("@oxide-fw/core/client/boot")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "@oxide-fw/core/client/boot".to_string());
    src.push_str(&format!(
        "import {{ bootWithRouter }} from {boot_fn_url:?};\n"
    ));
    // Island imports must come BEFORE the map — `import` at statement
    // position only. Emitting them inside the object literal is a syntax
    // error ("Unexpected token '{'" at the first island line).
    //
    // Island modules export `default` (standard React/Vue/Svelte component
    // convention), not a named `__island_Name` binding. Use the *default*
    // import form: `import __island_Name from "..."`. The previous named
    // form produced `import { __island_Name } from "..."` which the
    // browser rejects with "does not provide an export named
    // '__island_Name'" — the exact error a user sees on first page load.
    for (name, path) in islands {
        let sym = format!("__island_{name}");
        src.push_str(&format!("import {sym} from {path:?};\n"));
    }
    src.push_str("const __modules = {\n");
    for (name, _) in islands {
        let sym = format!("__island_{name}");
        src.push_str(&format!("  {name:?}: {sym},\n"));
    }
    src.push_str("};\n");

    src.push_str("if (typeof window !== 'undefined') {\n");
    src.push_str("  const __adapter = typeof clientAdapter !== 'undefined' ? clientAdapter : undefined;\n");
    src.push_str("  if (__adapter) {\n");
    // The chunk loader must prefer the HMR runtime's FRESH module record:
    // after a `module-update` re-imported a route module, the re-render
    // (`adapter.update` → `componentFor` → `loadChunk(componentUrl)`) has to
    // mount the NEW component — a plain `import(chunk)` on the un-busted URL
    // hits the browser ESM cache and hands back the STALE instance, so the
    // page re-renders the old tree and the edit appears to do nothing.
    // (boot.ts's ensureInstalled loader has the same check; this generated
    // entry is the loader a real ESM dev page actually uses.)
    src.push_str("    window.__oxideRegisterAdapter(__adapter, async (chunk) => {\n");
    src.push_str("      const hmr = globalThis.__oxide_hmr__;\n");
    src.push_str("      return __modules[chunk] ?? hmr?.getFreshModule?.(chunk) ?? (await import(chunk));\n");
    src.push_str("    });\n");
    src.push_str("    fetch('/@esm-manifest.json')\n");
    src.push_str("      .then(r => r.ok ? r.json() : undefined)\n");
    src.push_str("      .then(m => bootWithRouter(__adapter, { manifest: m, clientBase: '/' }))\n");
    src.push_str("      .catch(() => bootWithRouter(__adapter, { clientBase: '/' }));\n");
    src.push_str("  }\n");
    src.push_str("}\n");
    std::fs::write(&out, src)?;
    Ok(out)
}

// --- Rewrite map ---------------------------------------------------------

/// Bare specifier → URL. Built from the prebundle list (Phase 1b stub: files
/// missing until rolldown lands; the map is still correct so importing code
/// produces a clear 404 instead of a bare-specifier error) and the framework
/// package's `exports`.
pub fn build_rewrite_map(
    root: &Path,
    prebundle: &[String],
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    // One entry per package: the map is a straight 1:1 (bare specifier ->
    // its own prebundle URL). `react-dom/client` becomes
    // `/@prebundle/react-dom-client.js`, `react` becomes
    // `/@prebundle/react.js`, and so on. Cross-package imports inside the
    // prebundle files (e.g. `react-dom-client.js` -> `import React from
    // "react"`) are also rewritten to their canonical `/@prebundle/...` URL
    // by `rewrite_imports` when the file is served.
    for name in prebundle {
        map.insert(name.clone(), format!("/@prebundle/{}", sanitized_filename(name)));
    }
    // `react/jsx-dev-runtime` is imported by development-mode JSX lowering;
    // it ships in the same file as react/jsx-runtime (React's package.json
    // exports them to the same underlying implementation). Point it at the
    // same URL.
    if let Some(jsx_rt) = map.get("react/jsx-runtime").cloned() {
        map.entry("react/jsx-dev-runtime".to_string()).or_insert(jsx_rt);
    }
    // @oxide-fw/* packages use the framework exports scan (unchanged from
    // the prior version — those packages have real ESM dist files).
    if let Some(exports) = read_framework_exports(root) {
        for (specifier, url) in exports {
            map.insert(specifier, url);
        }
    }
    map
}

fn read_framework_exports(root: &Path) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();

    // 1. Scan node_modules/@oxide-fw/*
    let nm = root.join("node_modules/@oxide-fw");
    if nm.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&nm) {
            for entry in entries.flatten() {
                let pkg_json = entry.path().join("package.json");
                if let Some(mut exports) = read_exports_at(&pkg_json) {
                    out.append(&mut exports);
                }
            }
        }
    }

    // 2. Scan workspace packages/*/package.json
    let ws = root.join("packages");
    if ws.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&ws) {
            for entry in entries.flatten() {
                let pkg_json = entry.path().join("package.json");
                if let Some(mut exports) = read_exports_at(&pkg_json) {
                    out.append(&mut exports);
                }
            }
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

fn read_exports_at(pkg_json: &Path) -> Option<Vec<(String, String)>> {
    let raw = std::fs::read_to_string(pkg_json).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let name = json.get("name")?.as_str()?;
    if !name.starts_with("@oxide-fw/") { return None; }

    let exports = json.get("exports")?.as_object()?;
    let mut out = Vec::new();
    for (sub, entry) in exports {
        let sub_path = sub.trim_start_matches("./");
        let specifier = if sub_path.is_empty() || sub_path == "." {
            name.to_string()
        } else {
            format!("{name}/{sub_path}")
        };
        let target = entry
            .as_str()
            .or_else(|| entry.get("default").and_then(|v| v.as_str()));
        let Some(t) = target else { continue };
        let dist = t.trim_start_matches("./");
        // Map to the package's specific folder in the prebundle
        let pkg_dir = name.strip_prefix("@oxide-fw/").unwrap_or(name);
        out.push((
            specifier,
            format!("/@prebundle/@oxide-fw/{pkg_dir}/{dist}"),
        ));
    }
    Some(out)
}

// --- Prebundle -----------------------------------------------------------

/// Populate `.oxide/esm-prebundle/`.
///
/// **Phase 1a.5 (this function)**: copies the framework dist tree so
/// `@oxide-fw/*` imports resolve. React and other userland CJS deps are still
/// pending — requests to `/@prebundle/react.js` will 404, which surfaces the
/// missing Phase 1b step honestly. `dev.esm` defaults to `false`, so no
/// default user is affected.
///
/// **Phase 1b (next)**: bundle each `prebundle` entry from CJS to ESM using
/// `rolldown::Bundler`, writing one file per package name.
pub fn prebundle_cjs_deps(root: &Path, packages: &[String]) -> anyhow::Result<()> {
    let out_dir = root.join(PREBUNDLE_DIR);
    std::fs::create_dir_all(&out_dir)?;
    copy_framework_dist(root, &out_dir)?;

    if packages.is_empty() {
        return Ok(());
    }

    // rolldown lives in the bundler crate; cli is just the caller. A
    // per-package failure is logged but does not abort — a missing optional
    // dep must not break the dev server for a project that has the rest.
    let result = oxide_fw_bundler::prebundle_cjs_to_esm(root, packages, &out_dir)?;
    for (name, err) in result.failures {
        eprintln!(
            "oxide dev (esm): cannot prebundle `{name}` — /@prebundle/{} will 404 ({err})",
            sanitized_filename(&name)
        );
    }
    for name in result.bundled {
        tracing::debug!(package = %name, "esm prebundle ok");
    }
    Ok(())
}

fn copy_framework_dist(root: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let mut copied_any = false;

    let mut copy_pkg = |src: &Path, name: &str| -> anyhow::Result<()> {
        let dst = out_dir.join("@oxide-fw").join(name).join("dist");
        if dst.exists() {
            let _ = std::fs::remove_dir_all(&dst);
        }
        copy_dir_recursive(src, &dst)?;
        copied_any = true;
        Ok(())
    };

    if let Ok(entries) = std::fs::read_dir(root.join("node_modules/@oxide-fw")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let dist = entry.path().join("dist");
            if dist.is_dir() {
                let _ = copy_pkg(&dist, &name);
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(root.join("packages")) {
        for entry in entries.flatten() {
            let pkg_json = entry.path().join("package.json");
            if let Ok(raw) = std::fs::read_to_string(&pkg_json) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                        if name.starts_with("@oxide-fw/") {
                            let dist = entry.path().join("dist");
                            if dist.is_dir() {
                                let short = name.strip_prefix("@oxide-fw/").unwrap_or(name);
                                let _ = copy_pkg(&dist, short);
                            }
                        }
                    }
                }
            }
        }
    }

    if !copied_any {
        eprintln!(
            "oxide dev (esm): no @oxide-fw/* dist/ found — framework imports \
             will 404. Run `pnpm --filter @oxide-fw/core build` first."
        );
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn sanitized_filename(name: &str) -> String {
    // "react-dom/client" → "react-dom-client.js"
    // "react/jsx-runtime" → "react-jsx-runtime.js"
    // "lucide-react" → "lucide-react.js"
    let mut s: String = name.replace('/', "-").replace('@', "");
    s.push_str(".js");
    s
}

// --- Import rewriting ----------------------------------------------------

/// Rewrite every bare specifier in `code` that appears in `map` to its
/// `/@prebundle/...` URL. Relative and absolute specifiers are left alone
/// (relative imports resolve naturally in the browser; absolute URLs are
/// already correct).
fn rewrite_imports(
    code: &str,
    map: &BTreeMap<String, String>,
    from_src_rel: Option<&str>,
    src_root: Option<&std::path::Path>,
) -> String {
    let spans = oxide_fw_compiler::scan_import_spans(code);
    if spans.is_empty() {
        return code.to_string();
    }
    let mut out = code.to_string();
    // Reverse order so earlier offsets stay valid as we splice.
    for span in spans.iter().rev() {
        // Resolve the specifier to a replacement URL, in precedence order:
        //   1. bare specifier in the map → its prebundle URL
        //   2. relative CSS specifier when we know the importing file → /@css/
        let replacement: Option<String> = if let Some(pre) = map.get(&span.specifier) {
            // A relative specifier is never in the map (the map only holds
            // bare names). Keep the guard as a cheap invariant.
            if span.specifier.starts_with("./")
                || span.specifier.starts_with("../")
                || span.specifier.starts_with('/')
            {
                None
            } else {
                Some(pre.clone())
            }
        } else if is_server_specifier(&span.specifier) {
            from_src_rel
                .and_then(|from| resolve_server_url(from, &span.specifier))
        } else if is_css_specifier(&span.specifier) {
            from_src_rel
                .and_then(|from| resolve_css_url(from, &span.specifier))
        } else if is_js_specifier(&span.specifier) {
            from_src_rel.and_then(|from| {
                resolve_js_url_with_mtime(from, &span.specifier, src_root)
            })
        } else {
            None
        };
        let Some(replacement) = replacement else { continue };

        let slice_end = span.end.min(out.len());
        if span.start >= slice_end {
            continue;
        }
        let slice = &out[span.start..slice_end];
        for quote in ['\'', '"', '`'] {
            let needle = format!("{quote}{}{quote}", span.specifier);
            if let Some(pos) = slice.find(&needle) {
                let abs_start = span.start + pos + 1;
                let abs_end = abs_start + span.specifier.len();
                if abs_end <= out.len() {
                    out.replace_range(abs_start..abs_end, &replacement);
                }
                break;
            }
        }
    }
    out
}

/// True when a specifier points at a stylesheet. `.module.css` counts too —
/// plain-CSS modules are the current scope; `.scss`/`.sass` are accepted for
/// the same reason (a component that imports them gets the same treatment
/// until an actual preprocessor is wired).
/// §1.3: true when `spec` is a RELATIVE js/ts/tsx/jsx/mjs import — the
/// kind that stays verbatim in the served module and needs a `?t=<mtime>`
/// query so browser cache-busting works when the target changes.
/// `.vue`/`.svelte` count too: a page module compiled by the tooling pool
/// keeps its relative SFC imports verbatim, and when a CHILD component
/// changes the re-imported page module must fetch the child with a fresh
/// query or the browser's ESM cache serves the stale instance. The ESM
/// handler strips the query before dispatching (path_of), so the URLs are
/// served by the same pool-compile path.
fn is_js_specifier(spec: &str) -> bool {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return false;
    }
    let lower = spec.to_ascii_lowercase();
    lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".vue")
        || lower.ends_with(".svelte")
}

/// §1.3: resolve a relative js specifier to `/src/<rel>?t=<mtime>`. The
/// watcher publishes a ModuleUpdate for every importer of the changed
/// file; when that importer is re-imported, the fresh bytes contain the
/// new `?t=` here, so the browser re-evaluates the dep instead of serving
/// its cached module. Together these close the stale-binding gap.
fn resolve_js_url_with_mtime(
    from_src_rel: &str,
    spec: &str,
    src_root: Option<&std::path::Path>,
) -> Option<String> {
    let src_root = src_root?;
    let src_canon = std::fs::canonicalize(src_root).ok()?;
    let base = std::path::Path::new(from_src_rel)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let abs = std::fs::canonicalize(src_canon.join(base).join(spec)).ok()?;
    if !abs.starts_with(&src_canon) {
        return None;
    }
    let rel = abs.strip_prefix(&src_canon).ok()?;
    let mtime = std::fs::metadata(&abs)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    Some(format!("/src/{rel_str}?t={mtime}"))
}

fn is_css_specifier(spec: &str) -> bool {
    let lower = spec.to_ascii_lowercase();
    lower.ends_with(".css")
        || lower.ends_with(".scss")
        || lower.ends_with(".sass")
}

/// Turn a CSS specifier imported from `from_src_rel` (e.g.
/// `routes/layout.tsx`) into a `/@css/<src-relative-path>` URL. The
/// resolution is path-only: no filesystem canonicalize (the dev server has
/// no assumption about the file actually existing at that exact case on a
/// case-sensitive filesystem). A `../..` escape that would leave `src/` is
/// rejected — that import cannot be served and a full URL fallback would
/// leak host paths.
fn resolve_css_url(from_src_rel: &str, spec: &str) -> Option<String> {
    resolve_relative_url(from_src_rel, spec, CSS_URL_PREFIX)
}

/// Resolve a `.server.*` specifier imported from `from_src_rel` to a
/// `/@server-fn/<rel>` URL. Same path normalization as
/// [`resolve_css_url`] — a `../..` chain that escapes `src/` is rejected,
/// so a hostile import cannot probe outside the project source tree.
fn resolve_server_url(from_src_rel: &str, spec: &str) -> Option<String> {
    resolve_relative_url(from_src_rel, spec, SERVER_FN_URL_PREFIX)
}

/// Shared path normalization. Given a source-relative path and a relative
/// specifier, resolve one against the other and produce `<prefix><rel>`.
/// Rejects escapes past `src/`.
fn resolve_relative_url(from_src_rel: &str, spec: &str, prefix: &str) -> Option<String> {
    use std::path::Component;
    let from_dir = std::path::Path::new(from_src_rel)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let joined = from_dir.join(spec);
    let mut segments: Vec<std::ffi::OsString> = Vec::new();
    for comp in joined.components() {
        match comp {
            Component::Normal(s) => segments.push(s.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if segments.pop().is_none() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let rel = segments
        .iter()
        .map(|s| s.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Some(format!("{prefix}{rel}"))
}

/// True when the specifier names a server-only module (`.server.ts`,
/// `.server.tsx`, `.server.js`, `.server.jsx`, `.server.mjs`). Used by
/// `rewrite_imports` to route the import through `/@server-fn/` instead of
/// serving the raw file.
fn is_server_specifier(spec: &str) -> bool {
    let lower = spec.to_ascii_lowercase();
    lower.contains(".server.")
        && (lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
            || lower.ends_with(".mjs"))
}

// --- Request handler -----------------------------------------------------

/// Serve a request under `/src/`, `/@prebundle/`, `/@boot/`, or
/// `/@esm-error/`. Returns `None` for anything else so the caller falls
/// through to the normal pipeline.
///
/// `/src/*` requests for frameworks whose client transform lives in the
/// Node tooling pool (`.vue`, `.svelte`, Solid `.tsx`) come back as
/// [`EsmAction::PoolModule`] — the caller (the dispatch closure, which is
/// async) then awaits [`serve_pool_compiled_module`]. Every other match is
/// [`EsmAction::Ready`] and can be served synchronously as before.
pub fn handle_esm_request(state: &EsmDevState, req: &OxideRequest) -> Option<EsmAction> {
    let path = path_of(&req.url);
    if let Some(rest) = path.strip_prefix("/src/") {
        let clean = rest.split('?').next().unwrap_or(rest);
        if needs_pool_compile(state, clean) {
            return Some(EsmAction::PoolModule(clean.to_string()));
        }
        return Some(EsmAction::Ready(serve_source_file(state, clean)));
    }
    if let Some(rest) = path.strip_prefix("/@prebundle/") {
        return Some(EsmAction::Ready(serve_prebundle(state, rest)));
    }
    if let Some(rest) = path.strip_prefix(SERVER_FN_URL_PREFIX) {
        return Some(EsmAction::Ready(serve_server_fn_module(state, rest)));
    }
    if let Some(rest) = path.strip_prefix(CSS_URL_PREFIX) {
        return Some(EsmAction::Ready(serve_css_module(state, rest)));
    }
    if path == BOOT_SCRIPT_URL {
        return Some(EsmAction::Ready(serve_boot_entry(state)));
    }
    if path == "/@esm-manifest.json" {
        let manifest_path = state.root.join(MANIFEST_PATH);
        if let Ok(bytes) = std::fs::read(&manifest_path) {
            return Some(EsmAction::Ready(
                OxideResponse::new(200, bytes)
                    .with_header("content-type", "application/json")
                    .with_header("cache-control", "no-cache"),
            ));
        }
        return Some(EsmAction::Ready(OxideResponse::new(
            404,
            "esm manifest not generated",
        )));
    }
    if let Some(encoded) = path.strip_prefix("/@esm-error/") {
        return Some(EsmAction::Ready(serve_esm_error(encoded)));
    }
    None
}

fn path_of(url: &str) -> &str {
    let url = if let Some(rest) = url.strip_prefix("http://") {
        rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
    } else if let Some(rest) = url.strip_prefix("https://") {
        rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
    } else {
        url
    };
    let p = url.split_once('?').map(|(p, _)| p).unwrap_or(url);
    p.split_once('#').map(|(p, _)| p).unwrap_or(p)
}

fn serve_source_file(state: &EsmDevState, rel_path: &str) -> OxideResponse {
    // Refuse `.server.*`: those import `node:*` builtins that cannot run in a
    // browser. Serving them would hand arbitrary code filesystem access.
    if rel_path.contains(".server.") {
        return OxideResponse::new(403, "server module cannot be served to the client")
            .with_header("content-type", "text/plain; charset=utf-8")
            .with_header("cache-control", "no-cache");
    }
    // CSS/SCSS is handled by the asset pipeline (`<link>` in `<head>`). A
    // component's bare CSS import is handled in Phase 3 by rewriting it to a
    // throwing module; for now reject with a clear message.
    if rel_path.ends_with(".css")
        || rel_path.ends_with(".scss")
        || rel_path.ends_with(".sass")
    {
        return OxideResponse::new(404, "css asset cannot be served as esm")
            .with_header("content-type", "text/plain; charset=utf-8")
            .with_header("cache-control", "no-cache");
    }
    let abs = state.root.join("src").join(rel_path);
    let src = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(_) => {
            return OxideResponse::new(404, format!("source not found: src/{rel_path}"))
                .with_header("content-type", "text/plain; charset=utf-8");
        }
    };
    let lang = lang_for(rel_path);
    let path_for_compiler = format!("src/{rel_path}");

    // `lower_to_js_with_refresh` lowers JSX then injects `$RefreshReg$` calls
    // for every top-level component (a no-op on non-tsx/jsx sources).
    //
    // Note: server-fn stubbing (`transform_server_fn_stubs` with
    // `Env::Client`) runs in the `/@server-fn/` handler, not here. A page
    // that imports `.server.ts` has its import specifier rewritten by
    // `rewrite_imports` (below) to `/@server-fn/<rel>`, and that handler
    // produces the client stub. Running the transform here would be a
    // no-op (this file has no `serverFn(...)` definitions of its own).
    let lowered = match lower_to_js_with_refresh(
        &src,
        lang,
        &path_for_compiler,
        state.jsx_import_source.as_deref(),
    ) {
        Ok((code, _srcmap)) => code,
        Err(e) => {
            let body = format!(
                "// oxide dev: lower_to_js failed for src/{rel_path}\n\
                 throw new Error({:?});\n",
                format!("{} {}", e.0.code, e.0.message),
            );
            return OxideResponse::new(500, body)
                .with_header("content-type", "text/javascript; charset=utf-8")
                .with_header("cache-control", "no-cache");
        }
    };
    // `inject_hmr_boundary` no-ops when the source already contains
    // `import.meta.hot`; otherwise it appends the accept call. Runs before
    // rewriting so its `__oxide_refresh` call survives intact.
    let with_hmr = inject_hmr_boundary(&lowered, &path_for_compiler, lang)
        .unwrap_or(lowered);
    let rewritten = rewrite_imports(&with_hmr, &state.rewrite_map, Some(rel_path), Some(&state.root.join("src")));
    // Preamble: hot.ts auto-installs `globalThis.__oxide_hmr__` as a module
    // side-effect. The per-module `import.meta.hot` handle must be assigned
    // BEFORE the module body runs (the injected accept call reads it at top
    // level). ESM hoists imports, so the hot.ts import evaluates first, then
    // this file's top-level statements run.
    // §3.4: `import.meta.env` — Vite-parity. Read fresh per request so the
    // watcher's `.env*` branch only needs to publish FullReload (no
    // cross-scope RwLock). `oxide_fw_config::load` is ~1ms on this
    // project; acceptable in dev. Only `PUBLIC_*`-prefixed variables are
    // exposed (§14.2); everything else stays server-side. `MODE` is added
    // for Vite-parity.
    let env_json = {
        let mut obj = match oxide_fw_config::load(
            &state.root,
            "development",
            BTreeMap::new(),
        ) {
            Ok(loaded) => loaded.public_env(),
            Err(_) => BTreeMap::new(),
        };
        obj.insert("MODE".to_string(), "development".to_string());
        serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
    };
    let preamble = format!(
        "{}\
         Object.assign((import.meta.env || (import.meta.env = {{}})), {env_json});\n",
        hot_preamble(state, &format!("/src/{rel_path}")),
    );
    let out = format!("{preamble}{rewritten}");
    OxideResponse::new(200, out)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

/// Serve a CSS file imported from a route component as a JS module the
/// browser can load as an ESM. Shape:
///
/// ```js
/// // oxide-hmr: self-accept; re-import on change replaces the <style> tag
/// if (import.meta.hot) import.meta.hot.accept();
/// const id = "styles/tailwind.css";
/// const css = "<inlined text>";
/// let el = document.querySelector('style[data-oxide-css="' + id + '"]');
/// if (!el) { el = document.createElement("style"); el.dataset.oxideCss = id; document.head.appendChild(el); }
/// el.textContent = css;
/// export default css;
/// ```
///
/// When the CSS contains `@import "tailwindcss"` (or its single-quoted form),
/// the response is an empty stub — the actual CSS is delivered through the SSR
/// renderer's disk-scan `<link>` (`inject_route_css`), which is what makes the
/// tailwind precompute output reach the page. Returning the raw `@import` would
/// leave the browser trying to fetch a package name it cannot resolve.
/// Serve a `*.server.ts` module imported from a client component as a
/// `__oxide_call_fn` proxy.
///
/// This runs the **same** `transform_server_fn_stubs` pass that the
/// production build uses (`Env::Client`), so client-side behaviour in dev
/// matches production exactly:
///
/// ```ts
/// // src/routes/greeting.server.ts
/// export const greet = serverFn(async (n) => `hi ${n}`)
///
/// // becomes (served at /@server-fn/routes/greeting.server.ts)
/// import { __oxide_call_fn } from "@oxide-fw/core/fn"
/// export const greet = (async (...args) => __oxide_call_fn("<id>", args))
/// greet.id = "<id>"
/// ```
///
/// The `@oxide-fw/core/fn` import is then rewritten by `rewrite_imports`
/// to `/@prebundle/@oxide-fw/core/dist/fn.js`, which resolves to the
/// prebundled `__oxide_call_fn` that POSTs to `/_oxide/fn/<id>`.
///
/// This is Vite's server-reference pattern, not a "stub". The server
/// function's *body* never reaches the browser — only a network proxy.
fn serve_server_fn_module(state: &EsmDevState, rel_path: &str) -> OxideResponse {
    let abs = state.root.join("src").join(rel_path);
    let src = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(_) => {
            return OxideResponse::new(404, format!("server module not found: src/{rel_path}"))
                .with_header("content-type", "text/plain; charset=utf-8")
                .with_header("cache-control", "no-cache");
        }
    };
    // Same secret the prod build uses — so the client stub's `.id` matches
    // the SSR-side `.id` and hydration sees no mismatch. The dev server
    // exposes a dev secret via OXIDE_ROOT/.oxide/secret, but a stable
    // literal here is enough for dev: both server and client go through
    // the same transform in dev, so both see the same id.
    let (stubbed, _fns) = match oxide_fw_compiler::transform_server_fn_stubs(
        &src,
        &format!("src/{rel_path}"),
        oxide_fw_compiler::Env::Client,
        "dev-esm-stub-secret",
    ) {
        Ok(r) => r,
        Err(e) => {
            let body = format!(
                "throw new Error({:?});",
                format!("server-fn transform: {} {}", e.0.code, e.0.message),
            );
            return OxideResponse::new(500, body)
                .with_header("content-type", "text/javascript; charset=utf-8")
                .with_header("cache-control", "no-cache");
        }
    };
    // Rewrite `@oxide-fw/core/fn` → its prebundle URL.
    let rewritten = rewrite_imports(&stubbed, &state.rewrite_map, Some(rel_path), Some(&state.root.join("src")));
    OxideResponse::new(200, rewritten)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

fn serve_css_module(state: &EsmDevState, rel_path: &str) -> OxideResponse {
    let abs = state.root.join("src").join(rel_path);
    let source = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(_) => {
            return OxideResponse::new(404, format!("css not found: src/{rel_path}"))
                .with_header("content-type", "text/plain; charset=utf-8");
        }
    };
    // Tailwind entry: SSR-side disk-scan already emits the full generated
    // CSS via a <link>. The module itself is a no-op so the browser does not
    // try to fetch a bare `@import "tailwindcss"` specifier.
    let lower = source.to_ascii_lowercase();
    let has_tailwind_import = lower.contains("@import \"tailwindcss\"")
        || lower.contains("@import 'tailwindcss'");
    let (id, css_body) = if has_tailwind_import {
        (rel_path.to_string(), String::new())
    } else {
        (rel_path.to_string(), source)
    };

    // §1.4: CSS Modules. `x.module.css` gets scoped class names (lightningcss'
    // cssModules) and the served JS module exports the `local → mangled` map as
    // `default`. The style injection still runs (so HMR works). The SAME
    // module is what the SSR worker loads for a `.tsx` that imports
    // `./x.module.css`, so this one change fixes both client and SSR paths —
    // before, `heroStyles.hero` on the SSR side evaluated to
    // `("string").hero` = undefined (the CSS string's `.hero` property),
    // producing `<h1>` with no class attribute and no error.
    //
    // The plain-CSS path (`page.css`) is unchanged: it keeps `export default
    // css` (the string), which is what the current tests exercise.
    let is_module = rel_path.ends_with(".module.css")
        || rel_path.ends_with(".module.scss")
        || rel_path.ends_with(".module.sass");
    let (css_body_final, module_exports_js): (String, Option<String>) = if is_module && !css_body.is_empty() {
        use oxide_fw_assets::{CssBackend, CssInput, CssKind};
        let backend = oxide_fw_assets::LightningCssBackend;
        let input = CssInput {
            code: css_body.clone(),
            file: Some(format!("src/{rel_path}")),
            kind: CssKind::Module,
            targets: Vec::new(),
            minify: false,
        };
        match backend.transform(&input) {
            Ok(out) => {
                let exports_json = serde_json::to_string(&out.exports)
                    .unwrap_or_else(|_| "{}".into());
                (out.code, Some(exports_json))
            }
            Err(e) => {
                // A transform failure is a user-facing error; serve a module
                // whose default throws so the console shows the parse error
                // rather than a silent `{}` map.
                let msg = serde_json::to_string(&format!(
                    "oxide dev: CSS modules transform failed for {rel_path}: {} {}",
                    e.0.code, e.0.message
                )).unwrap_or_else(|_| "\"css modules transform failed\"".into());
                return OxideResponse::new(500, format!(
                    "throw new Error({msg});\nexport default {{}};\n"
                ))
                .with_header("content-type", "text/javascript; charset=utf-8")
                .with_header("cache-control", "no-cache");
            }
        }
    } else {
        (css_body, None)
    };
    // `serde_json::to_string` produces a JS-safe string literal (escaping
    // `"`, `\`, control chars); the CSS becomes a string, not raw code, so
    // no tag-break is possible.
    let css_literal = serde_json::to_string(&css_body_final).unwrap_or_else(|_| "\"\"".into());
    let id_literal = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".into());
    // Preamble: install `import.meta.hot` for THIS module (same pattern as
    // `serve_source_file`). Without it the `accept()` below is a no-op, the
    // module never enters `modules`, and `reloadModule("/@css/...")` cannot
    // find it to fire dispose/accept — CSS updates would be invisible until
    // a full page reload.
    let hot_url = state
        .rewrite_map
        .get("@oxide-fw/core/client/hot")
        .cloned()
        .unwrap_or_else(|| "@oxide-fw/core/client/hot".to_string());
    let self_url_literal = format!("{:?}", format!("{CSS_URL_PREFIX}{rel_path}"));
    let preamble = format!(
        "import {hot_url:?};\n\
         if (!import.meta.hot && globalThis.__oxide_hmr__) {{\n\
           import.meta.hot = globalThis.__oxide_hmr__.createHotContext({self_url_literal});\n\
         }}\n"
    );
    let export_line = match &module_exports_js {
        Some(json) => format!("export default {json};\n"),
        None => "export default css;\n".to_string(),
    };
    let body = format!(
        "{preamble}\
         // oxide-css: {rel_path}\n\
         if (import.meta.hot) import.meta.hot.accept();\n\
         const id = {id_literal};\n\
         const css = {css_literal};\n\
         if (css.length > 0 && typeof document !== 'undefined') {{\n\
           let el = document.querySelector('style[data-oxide-css=\"' + id + '\"]');\n\
           if (!el) {{ el = document.createElement('style'); el.dataset.oxideCss = id; document.head.appendChild(el); }}\n\
           el.textContent = css;\n\
         }}\n\
         {export_line}"
    );
    OxideResponse::new(200, body)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

fn serve_prebundle(state: &EsmDevState, rel_path: &str) -> OxideResponse {
    let abs = state.root.join(PREBUNDLE_DIR).join(rel_path);
    match std::fs::read(&abs) {
        Ok(bytes) => {
            let is_js = rel_path.ends_with(".js") || rel_path.ends_with(".mjs");
            let body = if is_js {
                match String::from_utf8(bytes) {
                    Ok(code) => rewrite_imports(&code, &state.rewrite_map, None, Some(&state.root.join("src"))).into_bytes(),
                    Err(e) => e.into_bytes(),
                }
            } else {
                bytes
            };
            OxideResponse::new(200, body)
                .with_header("content-type", "text/javascript; charset=utf-8")
                .with_header("cache-control", "no-cache")
        }
        Err(_) => OxideResponse::new(404, format!("prebundle not found: {rel_path}"))
            .with_header("content-type", "text/plain; charset=utf-8"),
    }
}

fn serve_boot_entry(state: &EsmDevState) -> OxideResponse {
    let abs = state.root.join(BOOT_ENTRY_PATH);
    match std::fs::read(&abs) {
        Ok(bytes) => OxideResponse::new(200, bytes)
            .with_header("content-type", "text/javascript; charset=utf-8")
            .with_header("cache-control", "no-cache"),
        Err(e) => OxideResponse::new(500, format!("boot entry missing: {e}"))
            .with_header("content-type", "text/plain; charset=utf-8"),
    }
}

fn serve_esm_error(encoded: &str) -> OxideResponse {
    let decoded = url_decode(encoded).unwrap_or_else(|| encoded.to_string());
    let body = format!(
        "throw new Error({:?});\n",
        format!("Failed to load module: {decoded}"),
    );
    OxideResponse::new(200, body)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

// --- Helpers -------------------------------------------------------------

fn lang_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "tsx" => "tsx",
        "jsx" => "jsx",
        "ts" | "mts" | "cts" => "ts",
        "js" | "mjs" | "cjs" => "js",
        _ => "ts",
    }
}

fn url_decode(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h = bytes.next()?;
            let l = bytes.next()?;
            let byte = (hex(h)? << 4) | hex(l)?;
            out.push(byte as char);
        } else {
            out.push(b as char);
        }
    }
    Some(out)
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_react() {
        assert_eq!(sanitized_filename("react"), "react.js");
        assert_eq!(sanitized_filename("react-dom/client"), "react-dom-client.js");
        assert_eq!(sanitized_filename("react/jsx-runtime"), "react-jsx-runtime.js");
        assert_eq!(sanitized_filename("@scope/pkg"), "scope-pkg.js");
    }

    #[test]
    fn rewrite_single_import() {
        let mut map = BTreeMap::new();
        map.insert("react".to_string(), "/@prebundle/react.js".to_string());
        let code = "import React from 'react';\nconsole.log(React);\n";
        let out = rewrite_imports(code, &map, None, None);
        assert!(out.contains("\"/@prebundle/react.js\"") || out.contains("'/@prebundle/react.js'"));
        assert!(!out.contains("from 'react'"));
    }

    #[test]
    fn rewrite_relative_untouched() {
        let mut map = BTreeMap::new();
        map.insert("./foo".to_string(), "/wrong".to_string());
        let code = "import x from './foo';\n";
        let out = rewrite_imports(code, &map, None, None);
        assert!(out.contains("from './foo'"));
    }

    #[test]
    fn rewrite_dynamic_import() {
        let mut map = BTreeMap::new();
        map.insert("react".to_string(), "/@prebundle/react.js".to_string());
        let code = "const r = await import('react');\n";
        let out = rewrite_imports(code, &map, None, None);
        assert!(out.contains("/@prebundle/react.js"));
        assert!(!out.contains("'react'"));
    }

    #[test]
    fn path_strips_query_and_fragment() {
        assert_eq!(path_of("/src/foo.tsx?v=1"), "/src/foo.tsx");
        assert_eq!(path_of("/src/foo.tsx#bar"), "/src/foo.tsx");
        assert_eq!(path_of("http://localhost:3000/src/foo.tsx"), "/src/foo.tsx");
        assert_eq!(path_of("/"), "/");
    }

    #[test]
    fn lang_mapping() {
        assert_eq!(lang_for("a.tsx"), "tsx");
        assert_eq!(lang_for("a.jsx"), "jsx");
        assert_eq!(lang_for("a.ts"), "ts");
        assert_eq!(lang_for("a.mjs"), "js");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    fn state(tmp: &Path) -> EsmDevState {
        let mut map = BTreeMap::new();
        map.insert(
            "@oxide-fw/core/client/hot".to_string(),
            "/@prebundle/@oxide-fw/core/dist/client-hot.js".to_string(),
        );
        map.insert("react".to_string(), "/@prebundle/react.js".to_string());
        EsmDevState {
            root: tmp.to_path_buf(),
            jsx_import_source: None,
            rewrite_map: Arc::new(map),
            framework: "react".to_string(),
        }
    }

    #[test]
    fn refuses_server_module_with_403() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let resp = serve_source_file(&s, "routes/page.server.ts");
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn missing_source_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let resp = serve_source_file(&s, "routes/page.tsx");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn css_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let resp = serve_source_file(&s, "styles/global.css");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn tsx_is_transformed_and_react_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/routes");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("page.tsx"),
            "import React from 'react';\nexport default function Page() {\n  return <div>hi</div>;\n}\n",
        )
        .unwrap();
        let s = state(tmp.path());
        let resp = serve_source_file(&s, "routes/page.tsx");
        assert_eq!(resp.status, 200, "expected 200, got {}", resp.status);
        let body = String::from_utf8_lossy(&resp.body[..]).into_owned();
        assert!(
            body.contains("/@prebundle/react.js"),
            "react import not rewritten: {body}"
        );
        assert!(
            !body.contains("from 'react'"),
            "bare react import survived: {body}"
        );
        assert!(
            body.contains("import.meta.hot"),
            "missing HMR preamble or accept call: {body}"
        );
        // Phase 3: React Refresh registration is injected for the component.
        assert!(
            body.contains("$RefreshReg$(Page"),
            "missing $RefreshReg$ for Page: {body}"
        );
        assert!(
            body.contains("const $RefreshReg$"),
            "missing $RefreshReg$ shim: {body}"
        );
    }

    #[test]
    fn prebundle_missing_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let resp = serve_prebundle(&s, "react.js");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn esm_error_returns_throwing_module() {
        let resp = serve_esm_error("some%20failed%20module");
        assert_eq!(resp.status, 200);
        let body = String::from_utf8_lossy(&resp.body[..]).into_owned();
        assert!(
            body.starts_with("throw new Error("),
            "expected throwing module, got: {body}"
        );
        assert!(body.contains("some failed module"));
    }

    #[test]
    fn build_manifest_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut components = BTreeMap::new();
        components.insert(
            "routes/index".to_string(),
            "/src/routes/page.tsx".to_string(),
        );
        build_esm_manifest(tmp.path(), &components).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join(MANIFEST_PATH)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["build_id"], "dev-esm");
        assert_eq!(
            v["routes"]["routes/index"]["component"],
            "/src/routes/page.tsx"
        );
    }

    #[test]
    fn prebundle_empty_packages_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        prebundle_cjs_deps(tmp.path(), &[]).unwrap();
        assert!(tmp.path().join(PREBUNDLE_DIR).is_dir());
    }

    #[test]
    fn css_import_resolves_to_at_css_url() {
        let mut map = BTreeMap::new();
        map.insert("react".into(), "/@prebundle/react.js".into());
        let code = "import React from 'react';\nimport './styles.css';\n";
        // from_src_rel is `routes/layout.tsx`; `./styles.css` resolves to
        // `routes/styles.css`.
        let out = rewrite_imports(code, &map, Some("routes/layout.tsx"), None);
        assert!(out.contains("/@css/routes/styles.css"), "css not rewritten: {out}");
        assert!(out.contains("/@prebundle/react.js"), "react not rewritten: {out}");
    }

    #[test]
    fn css_parent_escape_is_rejected() {
        let map = BTreeMap::new();
        // From src/page.tsx, `./x.css` → `x.css` (still under src/) → allowed.
        let code = "import './x.css';\n";
        let out = rewrite_imports(code, &map, Some("page.tsx"), None);
        assert!(out.contains("/@css/x.css"), "top-of-src import must resolve: {out}");

        // From src/a/b/c.tsx, `../../x.css` pops twice to src/x.css — allowed.
        let mid = "import '../../x.css';\n";
        let out_mid = rewrite_imports(mid, &map, Some("a/b/c.tsx"), None);
        assert!(out_mid.contains("/@css/x.css"), "mid-level pop should resolve: {out_mid}");

        // From src/a/b/c.tsx, `../../../x.css` pops three times → escapes src/
        // → rejected (specifier left untouched).
        let deep = "import '../../../x.css';\n";
        let out2 = rewrite_imports(deep, &map, Some("a/b/c.tsx"), None);
        assert!(
            out2.contains("../../../x.css"),
            "escape must not be rewritten: {out2}"
        );
    }

    #[test]
    fn css_module_serves_injectable_js() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        std::fs::create_dir_all(tmp.path().join("src/styles")).unwrap();
        std::fs::write(tmp.path().join("src/styles/plain.css"), "body { margin: 0; }").unwrap();
        let resp = serve_css_module(&s, "styles/plain.css");
        assert_eq!(resp.status, 200);
        let body = String::from_utf8_lossy(&resp.body[..]).into_owned();
        assert!(body.contains("import.meta.hot.accept"), "must self-accept: {body}");
        assert!(body.contains("body { margin: 0; }"), "css text must be inlined: {body}");
        assert!(body.contains("createElement('style')"), "must inject a style tag: {body}");
        // The preamble must install import.meta.hot with the module's own URL
        // so reloadModule can find it after a save.
        assert!(body.contains("createHotContext"), "must install import.meta.hot: {body}");
        assert!(
            body.contains("/@css/styles/plain.css"),
            "hot context URL must match the module: {body}"
        );
    }

    #[test]
    fn css_module_stubs_tailwind_import() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        std::fs::create_dir_all(tmp.path().join("src/styles")).unwrap();
        std::fs::write(
            tmp.path().join("src/styles/tailwind.css"),
            "@import \"tailwindcss\";\nbody { margin: 0; }\n",
        )
        .unwrap();
        let resp = serve_css_module(&s, "styles/tailwind.css");
        assert_eq!(resp.status, 200);
        let body = String::from_utf8_lossy(&resp.body[..]).into_owned();
        // The tailwind import is NOT inlined — the disk-scan <link> covers it.
        assert!(!body.contains("@import \"tailwindcss\""), "tailwind import must not survive: {body}");
        assert!(body.contains("import.meta.hot.accept"));
    }

    #[test]
    fn css_module_404_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let resp = serve_css_module(&s, "styles/missing.css");
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn boot_entry_references_prebundle_hot_url() {
        // Pure Rust — no server, no node_modules. `read_framework_exports`
        // falls back to `packages/core/package.json` relative to the temp
        // root; we write a minimal one carrying the `./client/hot` export
        // the boot entry depends on.
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("packages/core");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{
                "name": "@oxide-fw/core",
                "exports": {
                    "./client/hot": { "default": "./dist/client-hot.js" },
                    "./client": { "default": "./dist/client.js" }
                }
            }"#,
        )
        .unwrap();

        let mut components = BTreeMap::new();
        components.insert(
            "routes/index".to_string(),
            "/src/routes/page.tsx".to_string(),
        );
        let islands = BTreeMap::new();
        let boot_path =
            generate_esm_boot_entry(tmp.path(), &components, &islands, "react").unwrap();
        let body = std::fs::read_to_string(&boot_path).unwrap();
        // The boot file must import the prebundle URL, not the bare specifier
        // — the browser's native resolver cannot see node_modules.
        assert!(
            body.contains("/@prebundle/@oxide-fw/core/dist/client-hot.js"),
            "boot entry should reference the prebundle hot URL: {body}"
        );
        assert!(
            !body.contains("from \"@oxide-fw/core/client/hot\""),
            "bare specifier survived in boot entry: {body}"
        );
        assert!(
            body.contains("/src/routes/page.tsx"),
            "boot entry should import the page: {body}"
        );
    }

    #[test]
    fn boot_entry_wires_the_configured_framework_adapter() {
        // The ESM boot entry used to hard-code `@oxide-fw/react` — a Vue or
        // Solid or Svelte project got `__adapter = undefined`, no
        // bootWithRouter, no HMR socket. The entry must import
        // `clientAdapter` from the CONFIGURED framework's package.
        let tmp = tempfile::tempdir().unwrap();
        let packages = tmp.path().join("packages");
        // The boot entry's hot-runtime import requires @oxide-fw/core's
        // `./client/hot` export (same minimal package the hot-URL test
        // writes).
        let core = packages.join("core");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(
            core.join("package.json"),
            r#"{
                "name": "@oxide-fw/core",
                "exports": {
                    "./client/hot": { "default": "./dist/client-hot.js" },
                    "./client": { "default": "./dist/client.js" },
                    "./client/boot": { "default": "./dist/client-boot.js" }
                }
            }"#,
        )
        .unwrap();
        for fw in ["vue", "solid", "svelte"] {
            let pkg = packages.join(fw);
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(
                pkg.join("package.json"),
                format!(
                    r#"{{ "name": "@oxide-fw/{fw}", "exports": {{ ".": {{ "default": "./dist/index.js" }} }} }}"#
                ),
            )
            .unwrap();
        }
        let components = BTreeMap::new();
        let islands = BTreeMap::new();
        for fw in ["vue", "solid", "svelte"] {
            let boot_path =
                generate_esm_boot_entry(tmp.path(), &components, &islands, fw).unwrap();
            let body = std::fs::read_to_string(&boot_path).unwrap();
            let expected = format!("from \"/@prebundle/@oxide-fw/{fw}/dist/index.js\"");
            assert!(
                body.contains(&expected),
                "boot entry for {fw} must import its own clientAdapter (expected {expected}): {body}"
            );
            for other in ["react", "vue", "solid", "svelte"] {
                if other == fw {
                    continue;
                }
                assert!(
                    !body.contains(&format!("@oxide-fw/{other}/dist/index.js")),
                    "boot entry for {fw} must not import {other}'s adapter: {body}"
                );
            }
        }
    }

    #[test]
    fn needs_pool_compile_matches_sfc_and_solid_tsx_only() {
        let tmp = tempfile::tempdir().unwrap();
        let mut map = BTreeMap::new();
        map.insert(
            "@oxide-fw/core/client/hot".to_string(),
            "/@prebundle/@oxide-fw/core/dist/client-hot.js".to_string(),
        );
        let vue_state = EsmDevState {
            root: tmp.path().to_path_buf(),
            jsx_import_source: None,
            rewrite_map: Arc::new(map.clone()),
            framework: "vue".to_string(),
        };
        let solid_state = EsmDevState {
            root: tmp.path().to_path_buf(),
            jsx_import_source: Some("@solidjs/web".to_string()),
            rewrite_map: Arc::new(map.clone()),
            framework: "solid".to_string(),
        };
        let react_state = EsmDevState {
            root: tmp.path().to_path_buf(),
            jsx_import_source: None,
            rewrite_map: Arc::new(map),
            framework: "react".to_string(),
        };
        assert!(needs_pool_compile(&vue_state, "routes/index/page.vue"));
        assert!(needs_pool_compile(&vue_state, "routes/index/page.svelte"));
        assert!(!needs_pool_compile(&vue_state, "routes/index/page.tsx"));
        assert!(needs_pool_compile(&solid_state, "routes/index/page.tsx"));
        assert!(!needs_pool_compile(&solid_state, "lib/util.ts"));
        assert!(!needs_pool_compile(&react_state, "routes/index/page.tsx"));
        // SFC compilation is keyed by EXTENSION, not by the configured
        // framework (crates/compiler/src/framework.rs routes `.vue`/`.svelte`
        // through the tooling pool by design — there is no native SFC
        // parser). A React project that imports a `.vue` island still needs
        // the pool compile, so the vue/svelte arms are framework-agnostic.
        assert!(needs_pool_compile(&react_state, "routes/index/page.vue"));
        assert!(needs_pool_compile(&react_state, "routes/index/page.svelte"));
    }
}
