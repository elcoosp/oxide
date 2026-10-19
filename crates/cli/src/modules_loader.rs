//! §15-E load side: merge `oxide.module` packages into the build.
//!
//! The `add` command records a module name to `.oxide/modules.json` and
//! discovers an `oxide.module` field in the package's `package.json`. This
//! module reads those names back, resolves each package, and merges its
//! declared **routes** into the route tree the build scans. Hooks are the
//! follow-up (they need the JS plugin dispatch protocol pinned down; routes
//! are what a user sees working end-to-end first).
//!
//! Module contract (the subset honored here — extensions land as needed):
//!
//! ```json
//! {
//!   "name": "oxide-test-mod",
//!   "main": "./index.mjs",
//!   "oxide": {
//!     "module": {
//!       "routes": "./routes"   // optional; falls back to <pkg>/routes/
//!     }
//!   }
//! }
//! ```
//!
//! Routes come from either the explicit `routes` string (relative to the
//! package dir) or the `<package>/routes/` convention. Both scan exactly
//! like the project's own routes dir — page/layout/loading/endpoint entries
//! and their companion `*.server.ts` files are honored. Each entry's `file`
//! is rewritten to an absolute path under `node_modules/<name>/`, and each
//! `id` is namespaced under `modules/<name>/` so it cannot collide with a
//! project route. Downstream (bundler, prerender, dispatch) joins
//! `routes_root.join(<entry.file>)` — an absolute RHS short-circuits the
//! project prefix (PathBuf semantics), so module route files load from
//! their own package without any other plumbing.

use oxide_config::OxideConfig;
use oxide_router::{RouteTree};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub name: String,
    pub dir: PathBuf,
    pub routes_dir: Option<PathBuf>,
    /// Reserved for the hooks follow-up; the entry point the module's
    /// `oxide.module` hooks would be loaded from.
    pub entry: Option<PathBuf>,
    /// The hook names the module declares (`oxide.module.hooks`).
    pub hooks: Vec<String>,
}

impl ResolvedModule {
    /// True when the module contributes something the current loader
    /// honors. A module with only hooks declared currently contributes
    /// nothing (hooks are the follow-up) — report it as "not yet merged"
    /// so the build output is honest.
    pub fn routes_present(&self) -> bool {
        self.routes_dir.as_ref().is_some_and(|p| p.is_dir())
    }
}

/// Names read from `cfg.modules` plus `.oxide/modules.json`, deduped.
fn names_to_load(rootp: &Path, cfg: &OxideConfig) -> Vec<String> {
    let mut names: BTreeSet<String> = cfg.modules.iter().cloned().collect();
    let modules_json = rootp.join(".oxide/modules.json");
    if let Ok(bytes) = std::fs::read(&modules_json) {
        if let Ok(arr) = serde_json::from_slice::<Vec<String>>(&bytes) {
            for n in arr {
                names.insert(n);
            }
        }
    }
    names.into_iter().collect()
}

/// Read a module's `oxide.module` block from its `package.json`. Returns
/// `(routes_rel, entry_rel, hooks)` on success. A missing package, an
/// unreadable `package.json`, or a missing `oxide.module` field yields
/// `None` — a module that does not declare a contract is reported and
/// skipped (it is not an error: `add` records plain npm packages too).
fn read_contract(pkg_dir: &Path) -> Option<(Option<String>, Option<String>, Vec<String>)> {
    let pkg_json = pkg_dir.join("package.json");
    let bytes = std::fs::read(&pkg_json).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let module = v.get("oxide")?.get("module")?;

    let routes = module
        .get("routes")
        .and_then(|r| r.as_str())
        .map(str::to_string);
    let entry = module
        .get("entry")
        .and_then(|r| r.as_str())
        .map(str::to_string)
        .or_else(|| {
            // Fall back to package.json `main`, so a module that ships one
            // entry point does not have to declare it twice.
            v.get("main").and_then(|m| m.as_str()).map(str::to_string)
        });
    let hooks = module
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some((routes, entry, hooks))
}

/// Resolve every declared module against `<root>/node_modules/<name>/`.
/// Unresolvable or contract-less modules are skipped with a stderr warning;
/// the caller only ever sees modules that at least declare an
/// `oxide.module`.
pub fn load(rootp: &Path, cfg: &OxideConfig) -> Vec<ResolvedModule> {
    load_impl(rootp, cfg, true)
}

/// Same as [`load`], but with warnings suppressed. Callers that run inside
/// another loader (e.g. `build_js_plugin_hook`, called after
/// `scan_routes` has already warned once) use this to avoid duplicate
/// noise for the same module list.
pub fn load_quiet(rootp: &Path, cfg: &OxideConfig) -> Vec<ResolvedModule> {
    load_impl(rootp, cfg, false)
}

fn load_impl(rootp: &Path, cfg: &OxideConfig, warn: bool) -> Vec<ResolvedModule> {
    let mut out = Vec::new();
    for name in names_to_load(rootp, cfg) {
        let dir = rootp.join("node_modules").join(&name);
        if !dir.is_dir() {
            if warn {
                eprintln!("warn: module `{name}` is not installed at node_modules/{name}; skipping");
            }
            continue;
        }
        let Some((routes_rel, entry_rel, hooks)) = read_contract(&dir) else {
            if warn {
                eprintln!(
                    "warn: module `{name}` has no `oxide.module` field in package.json; skipping"
                );
            }
            continue;
        };
        let routes_dir = match routes_rel {
            Some(rel) => {
                let p = dir.join(rel);
                if p.is_dir() { Some(p) } else { None }
            }
            None => {
                let p = dir.join("routes");
                if p.is_dir() { Some(p) } else { None }
            }
        };
        let entry = entry_rel.map(|rel| dir.join(rel)).filter(|p| p.is_file());
        out.push(ResolvedModule {
            name,
            dir,
            routes_dir,
            entry,
            hooks,
        });
    }
    out
}

/// Scan each module's routes dir and merge the resulting entries into
/// `tree`. Returns the number of route files merged. Entries get a
/// `modules/<name>/` id prefix and an absolute `file` path; everything
/// downstream treats them like any other route (bundler chunks them,
/// prerender writes them, dispatch matches them).
pub fn inject_routes(tree: &mut RouteTree, mods: &[ResolvedModule]) -> usize {
    let mut total = 0usize;
    for m in mods {
        let Some(ref routes_dir) = m.routes_dir else {
            continue;
        };
        if !routes_dir.is_dir() {
            continue;
        }
        let subtree = match RouteTree::scan(routes_dir) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "warn: module `{}` routes scan failed: {} {}",
                    m.name, e.0.code, e.0.message
                );
                continue;
            }
        };
        let prefix = format!("modules/{}/", m.name);
        let mut count = 0usize;
        for mut entry in subtree.entries {
            // The subtree scan already produced a `segments` vector relative
            // to the module's own routes dir (e.g. `["blog", ":slug"]`). The
            // matcher keys on `segments`, so prepend the module namespace to
            // make the URL `/modules/<name>/blog/<slug>` — a module route
            // can never shadow a project route.
            let mut new_segs = vec!["modules".to_string(), m.name.clone()];
            new_segs.append(&mut entry.segments);
            entry.segments = new_segs;
            // Namespace the id too, so log output and manifests distinguish a
            // module route from a project route with the same shape.
            entry.id.0 = format!("{}{}", prefix, entry.id.0);
            // Absolute path so the bundler/prerender can join it directly.
            entry.file = routes_dir.join(&entry.file);
            tree.insert(entry);
            count += 1;
        }
        total += count;
    }
    total
}

/// True when the module declares hooks the loader does NOT yet merge.
/// `transform` is now wired (`build_js_plugin_hook`), so a module with
/// ONLY `transform` no longer triggers the notice. Any other hook name
/// still does — an honest signal that enabling it in config will not
/// make it fire yet.
pub fn has_unmerged_hooks(mods: &[ResolvedModule]) -> Vec<&ResolvedModule> {
    mods.iter()
        .filter(|m| m.hooks.iter().any(|h| h != "transform"))
        .collect()
}
