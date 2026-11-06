//! Prebundle CJS dependencies to ESM for the native ESM dev server.
//!
//! ## Why this exists
//!
//! Browsers can only execute ESM. React, react-dom, and most of the
//! ecosystem ship CJS entry points (`module.exports = require('./cjs/...')`).
//! Serving them raw produces a module with only `export default` — every
//! `import { useState } from 'react'` then fails at module-load time and
//! takes the whole module graph down with it.
//!
//! ## Vite's solution, applied here
//!
//! Rolldown's CJS-to-ESM interop (the same `@rollup/plugin-commonjs`-derived
//! analysis Rollup ships) statically scans `exports.X = ...` statements in
//! the CJS module and emits proper named ESM exports. **But it only runs
//! this analysis on bundle entry points.** A CJS module reached transitively
//! (e.g. via `export * from "react"` in a virtual entry) is wrapped in
//! `__commonJSMin` with no named exports — that's the trap our previous
//! virtual-entry approach fell into.
//!
//! So we do what Vite does: **one bundle per dependency, each as its own
//! entry**. Every package in `DevConfig::prebundle` gets its own file
//! `<sanitized-name>.js` in `.oxide/esm-prebundle/`. The Rolldown CJS
//! analysis runs on each entry, `exports.useState = ...` becomes
//! `export const useState = ...`, and the browser has real named ESM
//! exports.
//!
//! ## React's single-instance problem
//!
//! `react-dom-client` internally does `require('react')`. If we bundle
//! react into each dependent (naive), we ship **three** copies of React
//! (react.js, inside react-dom-client.js, inside lucide-react.js). Three
//! copies means three hook dispatchers, and `useState` from the wrong copy
//! throws "Invalid hook call" because react-dom's `createRoot()` never
//! populated that copy's dispatcher.
//!
//! The fix: mark every OTHER prebundle package as `external` when bundling
//! a given one. Rolldown then emits `import React from 'react'` (a bare
//! specifier) inside `react-dom-client.js`; our `rewrite_imports` pass in
//! `esm_dev.rs` rewrites that to `/@prebundle/react.js`. The browser
//! resolves all React imports to the same module — one instance, one
//! dispatcher.

use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct PrebundleResult {
    pub bundled: Vec<String>,
    pub failures: Vec<(String, String)>,
}

/// Bundle every entry in `packages` from CJS to ESM in `out_dir`, one file
/// per package. See the module docs for why one-per-package and why
/// inter-package externals.
pub fn prebundle_cjs_to_esm(
    root: &Path,
    packages: &[String],
    out_dir: &Path,
) -> anyhow::Result<PrebundleResult> {
    if packages.is_empty() {
        return Ok(PrebundleResult::default());
    }
    std::fs::create_dir_all(out_dir)?;

    // Resolve every entry up-front. `oxc_resolver` picks the ESM entry when
    // the package declares one (`module`, or `exports.import`), otherwise
    // the CJS entry (React 19 ships CJS-only — that's the point of the
    // Rolldown CJS interop analysis below).
    let resolver = oxc_resolver::Resolver::new(oxc_resolver::ResolveOptions {
        main_fields: vec!["module".to_string(), "main".to_string()],
        condition_names: vec![
            "import".to_string(),
            "module".to_string(),
            "browser".to_string(),
        ],
        ..oxc_resolver::ResolveOptions::default()
    });

    let mut resolved: Vec<(String, PathBuf)> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    for pkg in packages {
        match resolver.resolve(root, pkg) {
            Ok(r) => {
                // Follow the CJS re-export wrapper (`react/jsx-runtime.js`
                // -> `./cjs/react-jsx-runtime.development.js`) so Rolldown's
                // CJS interop can see the `exports.X = ...` statements and
                // emit real named ESM exports.
                let entry = resolve_through_wrapper(r.path());
                resolved.push((pkg.clone(), entry));
            }
            Err(e) => failures.push((pkg.clone(), format!("resolve: {e:?}"))),
        }
    }

    let all_names: Vec<String> = packages.to_vec();
    let mut bundled: Vec<String> = Vec::new();

    // Svelte's subpaths must share ONE runtime instance. The `svelte` entry
    // imports its internal client through RELATIVE paths
    // (`svelte/src/internal/client/index.js`), never through the bare
    // specifier `svelte/internal/client`, so the per-package
    // externalization cannot see the relationship: bundling `svelte`
    // separately INLINES a second copy of the client runtime. The page
    // module (compiled with `generate: 'dom'`) imports the standalone
    // `svelte/internal/client` prebundle — two copies means mount() sets
    // `component_context` on one module's state while the page's
    // lifecycle `init()` reads the other's (`null`) and the page falls back
    // to the IR renderer. A single multi-entry bundle (entries keep their
    // map-expected filenames, shared internals become one chunk) keeps
    // exactly one runtime instance across all of them.
    let svelte_family: Vec<(String, PathBuf)> = resolved
        .iter()
        .filter(|(pkg, _)| pkg == "svelte" || pkg.starts_with("svelte/"))
        .cloned()
        .collect();
    if svelte_family.len() > 1 {
        let externals: Vec<String> = all_names
            .iter()
            .filter(|p| !(p.as_str() == "svelte" || p.starts_with("svelte/")))
            .cloned()
            .collect();
        for (pkg, entry) in &svelte_family {
            if let Err(e) = bundle_family_multi_entry(
                root,
                &svelte_family,
                pkg,
                entry,
                out_dir,
                &externals,
            ) {
                failures.push((pkg.clone(), e));
                continue;
            }
            bundled.push(pkg.clone());
        }
    }

    for (pkg, entry) in &resolved {
        // The svelte family was already bundled together above.
        if svelte_family.iter().any(|(p, _)| p == pkg) {
            continue;
        }
        // External = every OTHER prebundle package. This makes react-dom-client
        // emit `import React from "react"` (bare, rewritten to
        // `/@prebundle/react.js` by `rewrite_imports` downstream) rather than
        // bundling its own React copy.
        let externals: Vec<String> = all_names
            .iter()
            .filter(|p| *p != pkg)
            .cloned()
            .collect();

        if let Err(e) = bundle_one(root, pkg, entry, out_dir, &externals) {
            failures.push((pkg.clone(), e));
            continue;
        }
        bundled.push(pkg.clone());
    }

    Ok(PrebundleResult {
        bundled,
        failures,
    })
}

/// Bundle a FAMILY of entries (one framework runtime + its subpaths) in a
/// single rolldown invocation so shared internals become ONE set of chunks
/// every entry imports — instead of each single-entry bundle inlining its
/// own copy. Entry chunks keep the names the prebundle URL map expects
/// (`sanitized_stem(pkg).js`); shared chunks get rolldown's hashed names and
/// are referenced relatively, which the browser resolves against the
/// `/@prebundle/...` URL.
///
/// `focus_pkg`/`focus_entry` select which entry's post-processing this call
/// performs (the CJS named-export augmentation + `__require` rewrite run per
/// entry chunk; svelte itself is pure ESM, so they normally no-op).
fn bundle_family_multi_entry(
    root: &Path,
    family: &[(String, PathBuf)],
    focus_pkg: &str,
    focus_entry: &Path,
    out_dir: &Path,
    externals: &[String],
) -> Result<(), String> {
    let focus_name = sanitized_stem(focus_pkg);
    let root_owned = root.to_path_buf();
    let out_dir_owned = out_dir.to_path_buf();
    let externals_owned = externals.to_vec();
    let inputs: Vec<rolldown::InputItem> = family
        .iter()
        .map(|(pkg, entry)| rolldown::InputItem {
            name: Some(sanitized_stem(pkg)),
            import: entry.to_string_lossy().into_owned(),
        })
        .collect();
    let focus_owned = focus_entry.to_path_buf();

    std::thread::scope(|scope| {
        scope
            .spawn(move || -> Result<(), String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("tokio: {e}"))?;

                rt.block_on(async move {
                    let external_opt = if externals_owned.is_empty() {
                        None
                    } else {
                        Some(rolldown::IsExternal::from(externals_owned.clone()))
                    };
                    let opts = rolldown::BundlerOptions {
                        input: Some(inputs),
                        cwd: Some(root_owned),
                        platform: Some(rolldown::Platform::Browser),
                        format: Some(rolldown::OutputFormat::Esm),
                        dir: Some(out_dir_owned.to_string_lossy().into_owned()),
                        external: external_opt,
                        define: Some(
                            [(
                                "process.env.NODE_ENV".to_string(),
                                "\"development\"".to_string(),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                        // Multi-entry + shared chunks: without splitting every
                        // entry would inline the whole runtime again.
                        code_splitting: Some(rolldown_common::CodeSplittingMode::Bool(true)),
                        ..Default::default()
                    };

                    let mut b = rolldown::BundlerBuilder::default()
                        .with_options(opts)
                        .build()
                        .map_err(|e| format!("init: {e}"))?;

                    b.write().await.map_err(|e| format!("write: {e}"))?;

                    // Same per-entry post-process as `bundle_one`, applied to
                    // THIS call's focus entry (each family member invokes
                    // bundle_family_multi_entry with itself as the focus).
                    let bundle_path = out_dir_owned.join(format!("{focus_name}.js"));
                    let names = std::fs::read_to_string(&focus_owned)
                        .map(|s| oxide_fw_compiler::extract_cjs_export_names(&s))
                        .unwrap_or_default();
                    let mut bundle = std::fs::read_to_string(&bundle_path)
                        .map_err(|e| format!("read bundle: {e}"))?;
                    let original_len = bundle.len();
                    if !names.is_empty() {
                        bundle = augment_bundle_with_named_exports(&bundle, &names);
                    }
                    bundle = rewrite_require_calls(&bundle, &externals_owned);
                    if bundle.len() != original_len {
                        std::fs::write(&bundle_path, &bundle)
                            .map_err(|e| format!("postprocess write: {e}"))?;
                    }
                    Ok(())
                })
            })
            .join()
            .unwrap_or_else(|_| Err("worker panicked".into()))
    })
}

fn bundle_one(
    root: &Path,
    pkg: &str,
    entry: &Path,
    out_dir: &Path,
    externals: &[String],
) -> Result<(), String> {
    let entry_name = sanitized_stem(pkg);
    let root_owned = root.to_path_buf();
    let entry_owned = entry.to_path_buf();
    let out_dir_owned = out_dir.to_path_buf();
    let externals_owned = externals.to_vec();

    std::thread::scope(|scope| {
        scope
            .spawn(move || -> Result<(), String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("tokio: {e}"))?;

                rt.block_on(async move {
                    // Set `external` so Rolldown does NOT bundle react into
                    // react-dom-client.js (that would ship three copies of
                    // React and break the hook dispatcher). The trade-off is
                    // that `require("react")` inside CJS becomes
                    // `__require("react")` — the browser-incompatible shim.
                    // The post-process below (`rewrite_require_calls`)
                    // converts those calls into top-level ESM imports, which
                    // the rewrite map resolves to /@prebundle/react.js. One
                    // React instance, no browser `require` call.
                    let external_opt = if externals_owned.is_empty() {
                        None
                    } else {
                        Some(rolldown::IsExternal::from(externals_owned.clone()))
                    };

                    let opts = rolldown::BundlerOptions {
                        input: Some(vec![rolldown::InputItem {
                            name: Some(entry_name.clone()),
                            import: entry_owned.to_string_lossy().into_owned(),
                        }]),
                        cwd: Some(root_owned),
                        platform: Some(rolldown::Platform::Browser),
                        format: Some(rolldown::OutputFormat::Esm),
                        dir: Some(out_dir_owned.to_string_lossy().into_owned()),
                        external: external_opt,
                        define: Some(
                            [(
                                "process.env.NODE_ENV".to_string(),
                                "\"development\"".to_string(),
                            )]
                            .into_iter()
                            .collect(),
                        ),
                        ..Default::default()
                    };

                    let mut b = rolldown::BundlerBuilder::default()
                        .with_options(opts)
                        .build()
                        .map_err(|e| format!("init: {e}"))?;

                    b.write()
                        .await
                        .map_err(|e| format!("write: {e}"))?;

                    // Post-process the bundle in one pass:
                    // (1) augment named exports extracted from the CJS AST
                    // (2) rewrite `__require("X")` to a top-level ESM import
                    let bundle_path = out_dir_owned.join(format!("{entry_name}.js"));
                    let names = std::fs::read_to_string(&entry_owned)
                        .map(|s| oxide_fw_compiler::extract_cjs_export_names(&s))
                        .unwrap_or_default();
                    let mut bundle = std::fs::read_to_string(&bundle_path)
                        .map_err(|e| format!("read bundle: {e}"))?;
                    let original_len = bundle.len();

                    if !names.is_empty() {
                        bundle = augment_bundle_with_named_exports(&bundle, &names);
                    }
                    bundle = rewrite_require_calls(&bundle, &externals_owned);

                    if bundle.len() != original_len {
                        std::fs::write(&bundle_path, &bundle)
                            .map_err(|e| format!("postprocess write: {e}"))?;
                        tracing::debug!(
                            entry = %entry_owned.display(),
                            from_bytes = original_len,
                            to_bytes = bundle.len(),
                            named_exports = names.len(),
                            "prebundle: postprocessed",
                        );
                    }
                    Ok(())
                })
            })
            .join()
            .unwrap_or_else(|_| Err("worker panicked".into()))
    })
}

// Follow a CJS re-export wrapper to the file that actually defines
// the exports. React's jsx-runtime.js wraps its real implementation
// in a `module.exports = require(...)` statement. Rolldown's CJS
// interop walks `exports.X = ...` in the entry file and cannot see
// through that wrapper, so it emits a bare default export. Following
// the wrapper first lets the interop see the real named exports.
//
// Only relative requires (the wrapper pattern) are followed. A peer
// dependency require (e.g. react inside react-dom-client) is left
// alone; that is externalization's job, not this helper's.
fn resolve_through_wrapper(path: &Path) -> PathBuf {
    resolve_through_wrapper_impl(path, 0)
}

fn resolve_through_wrapper_impl(path: &Path, depth: u32) -> PathBuf {
    if depth > 4 {
        return path.to_path_buf();
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return path.to_path_buf();
    };

    let mut candidates: Vec<String> = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("module.exports = require(") {
            let Some(quote) = rest.chars().next() else { continue };
            if !matches!(quote, '\'' | '"') {
                continue;
            }
            let Some(end) = rest[1..].find(quote) else { continue };
            let rel = &rest[1..1 + end];
            if let Some(stripped) = rel.strip_prefix("./") {
                candidates.push(stripped.to_string());
            }
        }
    }

    if candidates.is_empty() {
        return path.to_path_buf();
    }

    // Prefer the `.development.` variant so the prebundle has readable code
    // and React's dev warnings. Fall back to the first candidate.
    let chosen = candidates
        .iter()
        .find(|c| c.contains("development"))
        .or_else(|| candidates.first());

    let Some(rel) = chosen else {
        return path.to_path_buf();
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let target = parent.join(rel);
    if !target.is_file() {
        return path.to_path_buf();
    }
    // Recurse in case the target is also a wrapper.
    resolve_through_wrapper_impl(&target, depth + 1)
}

/// Given a Rolldown bundle that ends in `export default X;`, append named
/// exports for every name in `names` that the bundle doesn't already export.
///
/// Rolldown's built-in CJS-to-ESM interop extracts named exports only from
/// top-level `exports.X = ...` statements. React 19.3.1 wraps its 46
/// `exports.X = ...` assignments in a
/// `"production" !== process.env.NODE_ENV && (function(){...})();` IIFE, so
/// the built-in analysis emits a bare `export default require_react_development();`
/// with zero named exports. The browser then fails on every
/// `import { useState } from 'react'` and takes the module graph down with it.
///
/// This helper is the "post-process a bundle with the CJS analysis it should
/// have done" step that Vite performs after esbuild. The caller
/// (`bundle_one`) runs `extract_cjs_export_names` on the ORIGINAL source
/// (where the AST walk can see through the IIFE) and hands the resulting
/// names here. We rewrite the tail:
///
/// ```js
/// // before:
/// export default require_react_development();
///
/// // after:
/// var __oxide_cjs_default = require_react_development();
/// export default __oxide_cjs_default;
/// export const useState = __oxide_cjs_default.useState;
/// // ... etc for each missing name
/// ```
///
/// Idempotent: skips names the bundle already exports.
fn augment_bundle_with_named_exports(bundle: &str, names: &[String]) -> String {
    if names.is_empty() {
        return bundle.to_string();
    }
    let Some(default_idx) = bundle.rfind("export default ") else {
        return bundle.to_string();
    };
    let after = &bundle[default_idx + "export default ".len()..];
    let Some(semi_idx) = after.find(';') else {
        return bundle.to_string();
    };
    let expr = &after[..semi_idx];

    // Filter out names that the bundle already exports. This is a substring
    // check against the four forms Rolldown might have emitted; a false
    // positive here only costs us one missing export, and a false negative
    // is impossible (Rolldown emits one exact form per name).
    let mut missing: Vec<&String> = Vec::new();
    for name in names {
        let already = bundle.contains(&format!("export const {name} ="))
            || bundle.contains(&format!("export const {name} "))
            || bundle.contains(&format!("export function {name}("))
            || bundle.contains(&format!("export function {name} "))
            || bundle.contains(&format!("export class {name} "))
            || bundle.contains(&format!("export {{ {name} }}"))
            || bundle.contains(&format!("export {{ {name},"))
            || bundle.contains(&format!(", {name} }}"))
            || bundle.contains(&format!(", {name},"));
        if !already {
            missing.push(name);
        }
    }
    if missing.is_empty() {
        return bundle.to_string();
    }

    let tail = &bundle[default_idx + "export default ".len() + semi_idx..];

    let mut out = String::with_capacity(bundle.len() + missing.len() * 60 + 100);
    out.push_str(&bundle[..default_idx]);
    out.push_str(&format!("var __oxide_cjs_default = {expr};\n"));
    out.push_str(&format!("export default __oxide_cjs_default{tail}"));
    out.push_str("\n// oxide: named exports extracted from the CJS source by oxide-fw-bundler\n");
    for name in missing {
        out.push_str(&format!(
            "export const {name} = __oxide_cjs_default.{name};\n"
        ));
    }
    out
}

/// Rewrite `__require("pkg")` calls into top-level ESM imports.
///
/// ## Problem
///
/// Rolldown's CJS transform wraps a CJS module's `require('react')` in a
/// `__require("react")` shim. In a browser that shim throws:
///
/// ```text
/// Uncaught Error: Calling `require` for "react" in an environment that
/// doesn't expose the `require` function.
/// ```
///
/// `rolldown_plugin_esm_external_require` fixes this by converting the call
/// into a real `import ... from "react"` statement, but its only published
/// version (1.2.16) pulls `oxc 0.156`, which conflicts with rolldown
/// 1.2.15's `oxc 0.155`. Rather than drag in a second `oxc` tree, we do the
/// same transform on the bundle text.
///
/// For each external package name, replace `__require("pkg")` (and the
/// single-quoted form) with a fresh identifier, then prepend
/// `import <ident> from "pkg";`. The import specifier stays bare — the
/// dev server's `rewrite_imports` pass turns it into `/@prebundle/pkg.js`
/// at serve time, so every React import resolves to one module instance.
///
/// Only specifiers listed in `externals` are rewritten; a `require` for
/// anything else is left alone (and throws at runtime with a clear error).
fn rewrite_require_calls(bundle: &str, externals: &[String]) -> String {
    if externals.is_empty() {
        return bundle.to_string();
    }
    let mut out = bundle.to_string();
    let mut imports: Vec<(String, String)> = Vec::new();

    for (idx, ext) in externals.iter().enumerate() {
        let import_name = format!("__oxide_ext_{idx}");
        let dq = format!("__require(\"{ext}\")");
        let sq = format!("__require('{ext}')");
        let mut found = false;
        if out.contains(&dq) {
            out = out.replace(&dq, &import_name);
            found = true;
        }
        if out.contains(&sq) {
            out = out.replace(&sq, &import_name);
            found = true;
        }
        if found {
            imports.push((ext.clone(), import_name));
        }
    }

    if imports.is_empty() {
        return out;
    }

    let mut with_imports = String::with_capacity(out.len() + imports.len() * 48);
    for (module_id, import_name) in &imports {
        with_imports.push_str(&format!(
            "import {import_name} from {module_id:?};\n"
        ));
    }
    with_imports.push_str(&out);
    with_imports
}

/// `"react-dom/client"` → `"react-dom-client"` (the chunk stem — no `.js`,
/// Rolldown appends the extension).
fn sanitized_stem(name: &str) -> String {
    name.replace('/', "-").replace('@', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_package_names() {
        // The on-disk contract the dev server relies on: a prebundle entry is
        // `<sanitized_stem(pkg)>.js` and is mapped from `/@prebundle/<stem>.js`.
        let sanitized_filename = |name: &str| format!("{}.js", sanitized_stem(name));
        assert_eq!(sanitized_filename("react"), "react.js");
        assert_eq!(sanitized_filename("react-dom/client"), "react-dom-client.js");
        assert_eq!(sanitized_filename("react/jsx-runtime"), "react-jsx-runtime.js");
        assert_eq!(sanitized_filename("rangi/themes"), "rangi-themes.js");
    }

    #[test]
    fn empty_packages_yields_empty_result() {
        let tmp = tempfile::tempdir().unwrap();
        let r = prebundle_cjs_to_esm(tmp.path(), &[], &tmp.path().join("out")).unwrap();
        assert!(r.bundled.is_empty());
        assert!(r.failures.is_empty());
    }

    // The test above will not actually bundle (no node_modules in a tempdir);
    // the real bundling test is Playwright against a running dev server.
}
