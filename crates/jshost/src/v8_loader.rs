//! §4.2: a `ModuleLoader` for the V8 runtime that transpiles and resolves the
//! way the Node worker's `tsx-loader.mjs` does — but in Rust.
//!
//! `FsModuleLoader` (deno_core's) reads files verbatim, which fails on any
//! `.ts`/`.tsx`: V8 cannot parse type annotations or JSX. This loader reads the
//! file, runs `oxide_fw_compiler::lower_to_js` (Oxc type-strip + JSX lowering),
//! and returns the plain JS. It resolves a relative specifier against the
//! importer and a bare specifier against `node_modules` via `oxc_resolver` —
//! the same resolver the bundler uses, so a project that bundles also serves.
//!
//! This is what makes the V8 backend able to load `oxide.config.ts` and the
//! framework adapter it imports, not only a pre-bundled `dist/server` graph.

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleResolveResponse,
    ModuleSource, ModuleType, ResolutionKind,
};
use std::path::{Path, PathBuf};

/// The specifier a bare import resolves to on disk, plus its source.
pub struct TranspileLoader {
    /// Project root: bare specifiers resolve `node_modules` from here.
    root: PathBuf,
    /// The JSX runtime to lower `.tsx`/`.jsx` against (`react` → the Oxc
    /// default; `@solidjs/web` for Solid).
    jsx_import_source: Option<String>,
    /// PERF-10: the resolver, built **once** and reused. `oxc_resolver`'s
    /// `Resolver` holds caches (realpath, package.json, tsconfig); building it
    /// per resolve threw those away and re-read the filesystem every import.
    /// `OnceCell` because the loader is constructed before the process cwd is
    /// guaranteed stable.
    resolver: std::cell::OnceCell<oxc_resolver::Resolver>,
}

impl TranspileLoader {
    pub fn new(root: impl Into<PathBuf>, jsx_import_source: Option<String>) -> Self {
        Self {
            root: root.into(),
            jsx_import_source,
            resolver: std::cell::OnceCell::new(),
        }
    }

    /// PERF-10: the resolver, built on first use and reused thereafter.
    fn resolver(&self) -> &oxc_resolver::Resolver {
        self.resolver.get_or_init(|| {
            let options = oxc_resolver::ResolveOptions {
                extensions: vec![
                    ".ts".into(),
                    ".tsx".into(),
                    ".js".into(),
                    ".jsx".into(),
                    ".mjs".into(),
                    ".cjs".into(),
                    ".json".into(),
                ],
                // §4.6: the V8 runtime is browser-like. `worker` FIRST (see
                // the long note that used to live here): react-dom/server →
                // server.browser.js, @solidjs/web → dist/server.js.
                condition_names: vec![
                    "worker".into(),
                    "browser".into(),
                    "import".into(),
                    "module".into(),
                    "default".into(),
                ],
                main_fields: vec!["module".into(), "main".into()],
                ..Default::default()
            };
            oxc_resolver::Resolver::new(options)
        })
    }

    fn is_relative(spec: &str) -> bool {
        spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
    }

    /// Resolve a specifier seen from `referrer` (a `file://` URL) to a file
    /// path. Returns `None` if it cannot be resolved.
    fn resolve_to_path(&self, specifier: &str, referrer: &str) -> Option<PathBuf> {
        // A `file://` specifier (the framework ops import the config that way)
        // is already an absolute path — use it directly. Falling through to
        // oxc_resolver here is a race: it resolves inconsistently against a
        // base directory. An absolute path is the same case.
        // A `file://` specifier is already absolute — return it without an
        // `is_file` gate (a concurrent build write or a pool worker racing the
        // first render made the stat flaky). `load` reports a real read error.
        if let Some(path) = specifier.strip_prefix("file://") {
            return Some(PathBuf::from(path));
        }
        // A bare absolute path likewise.
        let as_path = Path::new(specifier);
        if as_path.is_absolute() {
            return Some(as_path.to_path_buf());
        }

        // The referrer is a `file://` URL; its directory is the resolution base.
        // Canonicalize it: a referrer like `<manifest>/../../project/.oxide/x.js`
        // leaves `..` segments that break a raw `join` probe on macOS, where
        // `/var` is a symlink to `/private/var` and a `..`-laden path can point
        // at a different file than its canonical form.
        let base_raw = referrer
            .strip_prefix("file://")
            .map(PathBuf::from)
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());
        let base_dir = std::fs::canonicalize(&base_raw).unwrap_or(base_raw);

        // A relative import with an explicit extension we can probe directly.
        if Self::is_relative(specifier) {
            let joined = base_dir.join(specifier);
            if let Some(found) = probe_extensions(&joined) {
                return Some(std::fs::canonicalize(&found).unwrap_or(found));
            }
        }

        // Everything else goes through the reused resolver (tsconfig paths,
        // package `exports`, conditions, node_modules) — PERF-10.
        self.resolver()
            .resolve(&base_dir, specifier)
            .ok()
            .map(|r| r.full_path().to_path_buf())
    }
}

/// Split `?query` off a specifier/URL. BUG-04 generation tags.
fn split_query(s: &str) -> (&str, Option<&str>) {
    match s.split_once('?') {
        Some((base, q)) => (base, Some(q)),
        None => (s, None),
    }
}

/// Try `p` as-is, then with each module extension appended.
fn probe_extensions(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let cand = p.with_extension(ext);
        if cand.is_file() {
            return Some(cand);
        }
    }
    // Directory index (`./dir` → `./dir/index.ts`).
    if p.is_dir() {
        for ext in ["ts", "tsx", "js", "jsx", "mjs"] {
            let cand = p.join(format!("index.{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// The source of a `node:*` shim (§4.6 tier 2). Covers the built-ins a
/// framework runtime references without depending on Node behavior: a no-op
/// `createRequire`, and minimal `path`/`fs`/`url` surfaces. Anything else
/// returns an empty module so an unused import does not fail the load.
fn node_shim(specifier: &str) -> String {
    // `specifier` is the URL path, e.g. `/shim/module`.
    let name = specifier.rsplit('/').next().unwrap_or("");
    match name {
        "module" => "export function createRequire() {\n\
             return function () { throw new Error('require() is not available in the V8 runtime'); };\n\
           }\n\
           export default { createRequire };\n"
            .to_string(),
        "path" => "export function join(...p) { return p.join('/').replace(/\\/+/g, '/'); }\n\
           export function resolve(...p) { return join(...p); }\n\
           export function dirname(p) { return p.replace(/\\/[^/]*$/, '') || '/'; }\n\
           export function basename(p) { return p.replace(/.*\\//, ''); }\n\
           export const sep = '/';\n\
           export default { join, resolve, dirname, basename, sep };\n"
            .to_string(),
        "url" => "export function fileURLToPath(u) { return String(u).replace(/^file:\\/\\//, '/'); }\n\
           export function pathToFileURL(p) { return new URL('file://' + (String(p).startsWith('/') ? '' : '/') + p); }\n\
           export default { fileURLToPath, pathToFileURL };\n"
            .to_string(),
        // SPEC-02 / §4.6.3: an unsupported Node builtin is LOUD. A silent
        // `export default {}` makes `import fs from 'node:fs'; fs.readFile(…)`
        // fail at the call site with a confusing "not a function" far from
        // the cause. Throwing on import names the builtin and the remedy.
        other => format!(
            "throw new Error('JSH-NODESHIM: node:{other} is not available in the V8 runtime; set server.backend to node or avoid this import (§4.6)');\n"
        ),
    }
}

/// SPEC-02 / §4.6.3: does this module specifier require the Node backend?
///
/// - a `.node` native addon — V8 cannot load it at all;
/// - a `node:` builtin outside the shim set (`fs`, `crypto`, `child_process`,
///   `net`, `worker_threads`, …).
///
/// When true and `server.backend` is `auto`/`v8`, the build switches to Node
/// and prints why.
pub fn requires_node_backend(specifier: &str) -> Option<String> {
    if specifier.ends_with(".node") {
        return Some(format!("native addon `{specifier}`"));
    }
    if let Some(name) = specifier.strip_prefix("node:") {
        let shimmed = matches!(
            name,
            "module" | "path" | "url" | "buffer" | "events" | "util" | "stream"
        );
        if !shimmed {
            return Some(format!("node:{name} builtin"));
        }
    }
    None
}

/// Pick the `lang` tag `lower_to_js` expects from a path's extension.
fn lang_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") => "ts",
        Some("tsx") => "tsx",
        Some("jsx") => "jsx",
        Some("js") => "js",
        Some("mjs") => "mjs",
        _ => "js",
    }
}

impl ModuleLoader for TranspileLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        // BUG-04: generation query (?v=N from __oxideFrameworkInvalidate).
        // Strip for path resolution, then re-attach to the canonical URL so
        // deno_core caches generations separately. A nested relative import
        // with no query inherits the referrer's — the whole graph moves to
        // the new generation together (mirrors the Node worker's versioned()
        // tagger).
        let (spec_base, spec_query) = split_query(specifier);
        let (_, ref_query) = split_query(referrer);
        let query = spec_query.or(ref_query);
        // §4.6 tier 2: a `node:*` specifier resolves to an in-memory shim, not
        // a file. V8 has no Node built-ins; the shim covers the handful a
        // framework runtime touches (a no-op `createRequire`, an empty
        // `path`/`fs`). Without this, bundling React's ESM build — which
        // imports `node:module` for a dev-only warning — fails at load.
        if let Some(builtin) = spec_base.strip_prefix("node:") {
            // Drop the colon: `oxide:shim/module`, so the shim's path is a
            // clean `/shim/<name>` with no second `:` to confuse URL parsing.
            let url = deno_core::ModuleSpecifier::parse(&format!("oxide:shim/{builtin}"))
                .map_err(|_| deno_error::JsErrorBox::generic("bad node: specifier"))?;
            return Ok(url);
        }
        let Some(path) = self.resolve_to_path(spec_base, referrer) else {
            return Err(deno_error::JsErrorBox::generic(format!(
                "cannot resolve `{specifier}` from `{referrer}`"
            )));
        };
        let mut url = deno_core::ModuleSpecifier::from_file_path(&path)
            .map_err(|_| deno_error::JsErrorBox::generic("path is not a file URL"))?;
        if let Some(q) = query {
            url.set_query(Some(q));
        }
        Ok(url)
    }

    fn load(
        &self,
        module_specifier: &deno_core::ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        // A `node:*` shim: return the in-memory source.
        if module_specifier.scheme() == "oxide" {
            let spec = module_specifier.clone();
            let src = node_shim(module_specifier.path());
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                deno_core::ModuleSourceCode::String(src.into()),
                &spec,
                None,
            )));
        }
        let (spec_no_query, _) = split_query(module_specifier.as_str());
        let path = match deno_core::ModuleSpecifier::parse(spec_no_query)
            .map_err(|_| deno_error::JsErrorBox::generic("module specifier is not a file path"))
            .and_then(|u| {
                u.to_file_path().map_err(|_| {
                    deno_error::JsErrorBox::generic("module specifier is not a file path")
                })
            }) {
            Ok(p) => p,
            Err(e) => {
                return ModuleLoadResponse::Sync(Err(e));
            }
        };
        // The ModuleSource keeps the ORIGINAL specifier (query included) so
        // deno_core caches `?v=N` generations as distinct modules (BUG-04).
        let spec = module_specifier.clone();
        // G1: retained as a field read for test introspection; the
        // compile path now routes through compile_module_sync.
        let _jsx = self.jsx_import_source.clone();
        let fut = async move {
            let src = std::fs::read_to_string(&path).map_err(|e| {
                deno_error::JsErrorBox::generic(format!("read {}: {e}", path.display()))
            })?;
            // BUG-08: CSS is a server side-effect import — resolve to an
            // empty module (SSR never applies styles; the client bundle's
            // CssPlugin handles the real CSS).
            if path.extension().and_then(|e| e.to_str()) == Some("css") {
                return Ok(ModuleSource::new(
                    ModuleType::JavaScript,
                    deno_core::ModuleSourceCode::String("export default \"\";".to_string().into()),
                    &spec,
                    None,
                ));
            }
            // JSON modules are passed through; V8 handles the `with { type }`.
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                return Ok(ModuleSource::new(
                    ModuleType::Json,
                    deno_core::ModuleSourceCode::String(src.into()),
                    &spec,
                    None,
                ));
            }
            let code = if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts" | "tsx" | "jsx")
            ) {
                let lang = lang_of(&path);
                let path_str = path.to_string_lossy().into_owned();
                let after_plugins = crate::v8_ops::compile_module_sync(
                    path_str.clone(),
                    src.clone(),
                    lang.to_string(),
                )
                .map_err(|e| {
                    deno_error::JsErrorBox::generic(format!(
                        "compile {}: {} {}",
                        path.display(), e.0.code, e.0.message
                    ))
                })?;
                oxide_fw_compiler::lower_to_js(
                    &after_plugins, lang, &path_str, _jsx.as_deref(),
                )
                .map(|(c, _map)| c)
                .map_err(|e| {
                    deno_error::JsErrorBox::generic(format!(
                        "lower {}: {} {}",
                        path.display(), e.0.code, e.0.message
                    ))
                })?
            } else {
                src
            };
            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                deno_core::ModuleSourceCode::String(code.into()),
                &spec,
                None,
            ))
        };
        ModuleLoadResponse::Async(Box::pin(fut))
    }
}
