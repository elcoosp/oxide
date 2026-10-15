//! §4.2: a `ModuleLoader` for the V8 runtime that transpiles and resolves the
//! way the Node worker's `tsx-loader.mjs` does — but in Rust.
//!
//! `FsModuleLoader` (deno_core's) reads files verbatim, which fails on any
//! `.ts`/`.tsx`: V8 cannot parse type annotations or JSX. This loader reads the
//! file, runs `oxide_compiler::lower_to_js` (Oxc type-strip + JSX lowering),
//! and returns the plain JS. It resolves a relative specifier against the
//! importer and a bare specifier against `node_modules` via `oxc_resolver` —
//! the same resolver the bundler uses, so a project that bundles also serves.
//!
//! This is what makes the V8 backend able to load `oxide.config.ts` and the
//! framework adapter it imports, not only a pre-bundled `dist/server` graph.

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader,
    ModuleResolveResponse, ModuleSource, ModuleType, ResolutionKind,
};
use std::path::{Path, PathBuf};

/// The specifier a bare import resolves to on disk, plus its source.
pub struct TranspileLoader {
    /// Project root: bare specifiers resolve `node_modules` from here.
    root: PathBuf,
    /// The JSX runtime to lower `.tsx`/`.jsx` against (`react` → the Oxc
    /// default; `@solidjs/web` for Solid).
    jsx_import_source: Option<String>,
}

impl TranspileLoader {
    pub fn new(root: impl Into<PathBuf>, jsx_import_source: Option<String>) -> Self {
        Self {
            root: root.into(),
            jsx_import_source,
        }
    }

    fn is_relative(spec: &str) -> bool {
        spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
    }

    /// Resolve a specifier seen from `referrer` (a `file://` URL) to a file
    /// path. Returns `None` if it cannot be resolved.
    fn resolve_to_path(&self, specifier: &str, referrer: &str) -> Option<PathBuf> {
        // The referrer is a `file://` URL; its directory is the resolution base.
        let base_dir = referrer
            .strip_prefix("file://")
            .map(PathBuf::from)
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());

        // A relative import with an explicit extension we can probe directly.
        if Self::is_relative(specifier) {
            let joined = base_dir.join(specifier);
            if let Some(found) = probe_extensions(&joined) {
                return Some(found);
            }
        }

        // Everything else goes through oxc_resolver (tsconfig paths, package
        // `exports`, conditions, node_modules). The same defaults the bundler
        // uses for the SSR environment.
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
            condition_names: vec!["node".into(), "import".into(), "default".into()],
            main_fields: vec!["module".into(), "main".into()],
            ..Default::default()
        };
        let resolver = oxc_resolver::Resolver::new(options);
        resolver
            .resolve(&base_dir, specifier)
            .ok()
            .map(|r| r.full_path().to_path_buf())
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
        _ => "export default {};\n".to_string(),
    }
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
        // §4.6 tier 2: a `node:*` specifier resolves to an in-memory shim, not
        // a file. V8 has no Node built-ins; the shim covers the handful a
        // framework runtime touches (a no-op `createRequire`, an empty
        // `path`/`fs`). Without this, bundling React's ESM build — which
        // imports `node:module` for a dev-only warning — fails at load.
        if let Some(builtin) = specifier.strip_prefix("node:") {
            // Drop the colon: `oxide:shim/module`, so the shim's path is a
            // clean `/shim/<name>` with no second `:` to confuse URL parsing.
            let url = deno_core::ModuleSpecifier::parse(&format!("oxide:shim/{builtin}"))
                .map_err(|_| deno_error::JsErrorBox::generic("bad node: specifier"))?;
            return Ok(url);
        }
        let Some(path) = self.resolve_to_path(specifier, referrer) else {
            return Err(deno_error::JsErrorBox::generic(format!(
                "cannot resolve `{specifier}` from `{referrer}`"
            )));
        };
        let url = deno_core::ModuleSpecifier::from_file_path(&path)
            .map_err(|_| deno_error::JsErrorBox::generic("path is not a file URL"))?;
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
        let path = match module_specifier.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                return ModuleLoadResponse::Sync(Err(deno_error::JsErrorBox::generic(
                    "module specifier is not a file path",
                )));
            }
        };
        let spec = module_specifier.clone();
        let jsx = self.jsx_import_source.clone();
        let fut = async move {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| deno_error::JsErrorBox::generic(format!("read {}: {e}", path.display())))?;
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
                let (js, _map) = oxide_compiler::lower_to_js(
                    &src,
                    lang,
                    &path.to_string_lossy(),
                    jsx.as_deref(),
                )
                .map_err(|e| {
                    deno_error::JsErrorBox::generic(format!(
                        "lower {}: {} {}",
                        path.display(),
                        e.0.code,
                        e.0.message
                    ))
                })?;
                js
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
