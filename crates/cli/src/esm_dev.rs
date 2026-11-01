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

/// Everything the ESM request handler needs. Built once at dev startup,
/// shared through `Arc` with the dispatch closure.
pub struct EsmDevState {
    pub root: PathBuf,
    pub jsx_import_source: Option<String>,
    /// Bare specifier → `/@prebundle/...` URL. Used both to rewrite the
    /// imports of served files and to compose the boot entry.
    pub rewrite_map: Arc<BTreeMap<String, String>>,
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
    for name in prebundle {
        map.insert(name.clone(), format!("/@prebundle/{}", sanitized_filename(name)));
    }
    if let Some(exports) = read_framework_exports(root) {
        for (specifier, url) in exports {
            map.insert(specifier, url);
        }
    }
    map
}

fn read_framework_exports(root: &Path) -> Option<Vec<(String, String)>> {
    let candidates = [
        root.join("node_modules/@oxide-fw/core/package.json"),
        root.join("packages/core/package.json"),
    ];
    for pkg in &candidates {
        if let Some(exports) = read_exports_at(pkg) {
            return Some(exports);
        }
    }
    None
}

fn read_exports_at(pkg_json: &Path) -> Option<Vec<(String, String)>> {
    let raw = std::fs::read_to_string(pkg_json).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let exports = json.get("exports")?.as_object()?;
    let mut out = Vec::new();
    for (sub, entry) in exports {
        let sub_path = sub.trim_start_matches("./");
        let specifier = if sub_path.is_empty() || sub_path == "." {
            "@oxide-fw/core".to_string()
        } else {
            format!("@oxide-fw/core/{sub_path}")
        };
        let target = entry
            .as_str()
            .or_else(|| entry.get("default").and_then(|v| v.as_str()));
        let Some(t) = target else { continue };
        let dist = t.trim_start_matches("./");
        out.push((
            specifier,
            format!("/@prebundle/@oxide-fw/core/{dist}"),
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
    let candidates = [
        root.join("node_modules/@oxide-fw/core/dist"),
        root.join("packages/core/dist"),
    ];
    let Some(src) = candidates.iter().find(|p| p.is_dir()) else {
        eprintln!(
            "oxide dev (esm): no `packages/core/dist/` found — @oxide-fw/* imports \
             will 404. Run `pnpm --filter @oxide-fw/core build` first."
        );
        return Ok(());
    };
    let dst = out_dir.join("@oxide-fw/core/dist");
    if dst.exists() {
        let _ = std::fs::remove_dir_all(&dst);
    }
    copy_dir_recursive(src, &dst)?;
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
        } else if is_css_specifier(&span.specifier) {
            from_src_rel
                .and_then(|from| resolve_css_url(from, &span.specifier))
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
    use std::path::Component;
    let from_dir = std::path::Path::new(from_src_rel)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let joined = from_dir.join(spec);
    // Normalize: drop `.` and pop `..`, rejecting anything that escapes.
    let mut segments: Vec<std::ffi::OsString> = Vec::new();
    for comp in joined.components() {
        match comp {
            Component::Normal(s) => segments.push(s.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if segments.pop().is_none() {
                    return None; // escaped src/
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
    Some(format!("{CSS_URL_PREFIX}{rel}"))
}

// --- Request handler -----------------------------------------------------

/// Serve a request under `/src/`, `/@prebundle/`, `/@boot/`, or
/// `/@esm-error/`. Returns `None` for anything else so the caller falls
/// through to the normal pipeline.
pub fn handle_esm_request(
    state: &EsmDevState,
    req: &OxideRequest,
) -> Option<OxideResponse> {
    let path = path_of(&req.url);
    if let Some(rest) = path.strip_prefix("/src/") {
        return Some(serve_source_file(state, rest));
    }
    if let Some(rest) = path.strip_prefix("/@prebundle/") {
        return Some(serve_prebundle(state, rest));
    }
    if let Some(rest) = path.strip_prefix(CSS_URL_PREFIX) {
        return Some(serve_css_module(state, rest));
    }
    if path == BOOT_SCRIPT_URL {
        return Some(serve_boot_entry(state));
    }
    if let Some(encoded) = path.strip_prefix("/@esm-error/") {
        return Some(serve_esm_error(encoded));
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
    let rewritten = rewrite_imports(&with_hmr, &state.rewrite_map, Some(rel_path));
    // Preamble: hot.ts auto-installs `globalThis.__oxide_hmr__` as a module
    // side-effect. The per-module `import.meta.hot` handle must be assigned
    // BEFORE the module body runs (the injected accept call reads it at top
    // level). ESM hoists imports, so the hot.ts import evaluates first, then
    // this file's top-level statements run.
    let hot_url = state
        .rewrite_map
        .get("@oxide-fw/core/client/hot")
        .cloned()
        .unwrap_or_else(|| "@oxide-fw/core/client/hot".to_string());
    let url_literal = format!("{:?}", format!("/src/{rel_path}"));
    let preamble = format!(
        "import {hot_url:?};\n\
         if (!import.meta.hot && globalThis.__oxide_hmr__) {{\n\
           import.meta.hot = globalThis.__oxide_hmr__.createHotContext({url_literal});\n\
         }}\n"
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
    // `serde_json::to_string` produces a JS-safe string literal (escaping
    // `"`, `\`, control chars); the CSS becomes a string, not raw code, so
    // no tag-break is possible.
    let css_literal = serde_json::to_string(&css_body).unwrap_or_else(|_| "\"\"".into());
    let id_literal = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".into());
    let body = format!(
        "// oxide-css: {rel_path}\n\
         if (import.meta.hot) import.meta.hot.accept();\n\
         const id = {id_literal};\n\
         const css = {css_literal};\n\
         if (css.length > 0 && typeof document !== 'undefined') {{\n\
           let el = document.querySelector('style[data-oxide-css=\"' + id + '\"]');\n\
           if (!el) {{ el = document.createElement('style'); el.dataset.oxideCss = id; document.head.appendChild(el); }}\n\
           el.textContent = css;\n\
         }}\n\
         export default css;\n"
    );
    OxideResponse::new(200, body)
        .with_header("content-type", "text/javascript; charset=utf-8")
        .with_header("cache-control", "no-cache")
}

fn serve_prebundle(state: &EsmDevState, rel_path: &str) -> OxideResponse {
    let abs = state.root.join(PREBUNDLE_DIR).join(rel_path);
    match std::fs::read(&abs) {
        Ok(bytes) => {
            // JS/MJS: rewrite bare specifiers. Non-JS: serve as-is (the
            // prebundle tree should only contain JS today, but be tolerant).
            let is_js = rel_path.ends_with(".js") || rel_path.ends_with(".mjs");
            let body = if is_js {
                match String::from_utf8(bytes) {
                    Ok(code) => rewrite_imports(&code, &state.rewrite_map, None).into_bytes(),
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
        let out = rewrite_imports(code, &map, None);
        assert!(out.contains("\"/@prebundle/react.js\"") || out.contains("'/@prebundle/react.js'"));
        assert!(!out.contains("from 'react'"));
    }

    #[test]
    fn rewrite_relative_untouched() {
        let mut map = BTreeMap::new();
        map.insert("./foo".to_string(), "/wrong".to_string());
        let code = "import x from './foo';\n";
        let out = rewrite_imports(code, &map, None);
        assert!(out.contains("from './foo'"));
    }

    #[test]
    fn rewrite_dynamic_import() {
        let mut map = BTreeMap::new();
        map.insert("react".to_string(), "/@prebundle/react.js".to_string());
        let code = "const r = await import('react');\n";
        let out = rewrite_imports(code, &map, None);
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
        let out = rewrite_imports(code, &map, Some("routes/layout.tsx"));
        assert!(out.contains("/@css/routes/styles.css"), "css not rewritten: {out}");
        assert!(out.contains("/@prebundle/react.js"), "react not rewritten: {out}");
    }

    #[test]
    fn css_parent_escape_is_rejected() {
        let map = BTreeMap::new();
        // From src/page.tsx, `./x.css` → `x.css` (still under src/) → allowed.
        let code = "import './x.css';\n";
        let out = rewrite_imports(code, &map, Some("page.tsx"));
        assert!(out.contains("/@css/x.css"), "top-of-src import must resolve: {out}");

        // From src/a/b/c.tsx, `../../x.css` pops twice to src/x.css — allowed.
        let mid = "import '../../x.css';\n";
        let out_mid = rewrite_imports(mid, &map, Some("a/b/c.tsx"));
        assert!(out_mid.contains("/@css/x.css"), "mid-level pop should resolve: {out_mid}");

        // From src/a/b/c.tsx, `../../../x.css` pops three times → escapes src/
        // → rejected (specifier left untouched).
        let deep = "import '../../../x.css';\n";
        let out2 = rewrite_imports(deep, &map, Some("a/b/c.tsx"));
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
        let boot_path = generate_esm_boot_entry(tmp.path(), &components).unwrap();
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
}
