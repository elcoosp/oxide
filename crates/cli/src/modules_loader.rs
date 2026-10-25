//! §15-E load side: merge `oxide.module` packages into the build.
//!
//! The `add` command records a module name to `.oxide/modules.json` and
//! discovers an `oxide.module` field in the package's `package.json`. This
//! module reads those names back, resolves each package, merges its
//! declared **routes** into the route tree the build scans, and surfaces
//! its **transform** hook into the JS plugin chain. Everything else the
//! contract supports (`adapters`, `typegen`, `configDefaults`) is
//! *resolved* and *reported* — merging those into the pipeline is the
//! documented follow-up.
//!
//! Module contract (the subset honored today):
//!
//! ```json
//! {
//!   "name": "oxide-test-mod",
//!   "main": "./index.mjs",
//!   "oxide": {
//!     "module": {
//!       "routes": "./routes",
//!       "hooks": ["transform"],
//!       "adapters": ["node"],
//!       "typegen": "./types.d.ts",
//!       "configDefaults": { "images": { "remotePatterns": [] } }
//!     }
//!   }
//! }
//! ```
//!
//! Routes come from either the explicit `routes` string (relative to the
//! package dir) or the `<package>/routes/` convention. Both scan exactly
//! like the project's own routes dir — page/layout/loading/endpoint entries
//! and their companion `*.server.ts` files are honored. Each entry's `file`
//! is rewritten to an absolute path under `node_modules/<name>/`, each
//! `segments` vector is prefixed with `["modules", "<name>"]` so the URL is
//! `/modules/<name>/…`, and each `id` is namespaced under `modules/<name>/`
//! so it cannot collide with a project route. Downstream (bundler,
//! prerender, dispatch) joins `routes_root.join(<entry.file>)` — an
//! absolute RHS short-circuits the project prefix (PathBuf semantics), so
//! module route files load from their own package without any other
//! plumbing.
//!
//! Hooks: `transform` is merged (the module's `entry` runs through the same
//! `plugin:call` machinery a `kind:js` plugin uses); any other hook name is
//! reported at `add` time and once at build time so a module declaring it
//! is not a silent dead end.

use oxide_fw_config::OxideConfig;
use oxide_fw_router::RouteTree;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The `oxide.module` contract, read from a package's `package.json`.
/// Every field is optional; a package with no `oxide.module` yields `None`
/// from [`read_contract`].
#[derive(Debug, Clone, Default)]
pub struct Contract {
    pub routes: Option<String>,
    pub entry: Option<String>,
    pub hooks: Vec<String>,
    pub adapters: Vec<String>,
    pub typegen: Option<String>,
    pub config_defaults: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub name: String,
    pub dir: PathBuf,
    pub routes_dir: Option<PathBuf>,
    /// Reserved for the hooks follow-up; the entry point a `transform` (or
    /// future hook) runs from.
    pub entry: Option<PathBuf>,
    /// Hook names the module declares. `transform` is merged; any other
    /// name is reported as deferred.
    pub hooks: Vec<String>,
    /// Declared adapter compatibility (`["node", "binary", …]`). Empty =
    /// no restriction.
    pub adapters: Vec<String>,
    /// A `.d.ts` the module ships. Reported but not merged — wiring it into
    /// `.oxide/types/` is a documented follow-up.
    pub typegen: Option<PathBuf>,
    /// Default config values keyed by config path. Reported but not merged —
    /// merging under `modules.<name>` is a documented follow-up.
    pub config_defaults: Option<serde_json::Value>,
}

impl ResolvedModule {
    /// True when the module contributes something the loader honors.
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

/// Read a module's `oxide.module` block from its `package.json`. A missing
/// package, an unreadable `package.json`, or a missing `oxide.module` field
/// yields `None` — a module that does not declare a contract is reported
/// and skipped (it is not an error: `add` records plain npm packages too).
fn read_contract(pkg_dir: &Path) -> Option<Contract> {
    let pkg_json = pkg_dir.join("package.json");
    let bytes = std::fs::read(&pkg_json).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let module = v.get("oxide")?.get("module")?;

    Some(Contract {
        routes: module
            .get("routes")
            .and_then(|r| r.as_str())
            .map(str::to_string),
        entry: module
            .get("entry")
            .and_then(|r| r.as_str())
            .map(str::to_string)
            .or_else(|| v.get("main").and_then(|m| m.as_str()).map(str::to_string)),
        hooks: module
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        adapters: module
            .get("adapters")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        typegen: module
            .get("typegen")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        config_defaults: module.get("configDefaults").cloned(),
    })
}

/// Resolve a single package dir to a `ResolvedModule`. Public so `oxide add`
/// can describe a module without running the full project scan.
pub fn resolve_one(pkg_dir: &Path, name: String) -> Option<ResolvedModule> {
    let c = read_contract(pkg_dir)?;
    let routes_dir = match c.routes {
        Some(ref rel) => {
            let p = pkg_dir.join(rel);
            if p.is_dir() { Some(p) } else { None }
        }
        None => {
            let p = pkg_dir.join("routes");
            if p.is_dir() { Some(p) } else { None }
        }
    };
    let entry = c.entry.map(|rel| pkg_dir.join(rel)).filter(|p| p.is_file());
    let typegen = c
        .typegen
        .map(|rel| pkg_dir.join(rel))
        .filter(|p| p.is_file());
    Some(ResolvedModule {
        name,
        dir: pkg_dir.to_path_buf(),
        routes_dir,
        entry,
        hooks: c.hooks,
        adapters: c.adapters,
        typegen,
        config_defaults: c.config_defaults,
    })
}

/// Resolve every declared module against `<root>/node_modules/<name>/`.
/// Unresolvable or contract-less modules are skipped with a stderr warning;
/// the caller only ever sees modules that at least declare an
/// `oxide.module`.
pub fn load(rootp: &Path, cfg: &OxideConfig) -> Vec<ResolvedModule> {
    load_impl(rootp, cfg, true)
}

/// Same as [`load`], but with warnings suppressed. Callers that run inside
/// another loader (e.g. `build_js_plugin_hook`, called after `scan_routes`
/// has already warned once) use this to avoid duplicate noise for the same
/// module list.
pub fn load_quiet(rootp: &Path, cfg: &OxideConfig) -> Vec<ResolvedModule> {
    load_impl(rootp, cfg, false)
}

fn load_impl(rootp: &Path, cfg: &OxideConfig, warn: bool) -> Vec<ResolvedModule> {
    let mut out = Vec::new();
    for name in names_to_load(rootp, cfg) {
        let dir = rootp.join("node_modules").join(&name);
        if !dir.is_dir() {
            if warn {
                eprintln!(
                    "warn: module `{name}` is not installed at node_modules/{name}; skipping"
                );
            }
            continue;
        }
        let Some(m) = resolve_one(&dir, name) else {
            if warn {
                eprintln!(
                    "warn: module `{}` has no `oxide.module` field in package.json; skipping",
                    dir.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("(unknown)"),
                );
            }
            continue;
        };
        out.push(m);
    }
    out
}

/// Scan each module's routes dir and merge the resulting entries into
/// `tree`. Returns the number of route files merged.
pub fn inject_routes(tree: &mut RouteTree, mods: &[ResolvedModule]) -> usize {
    let mut total = 0usize;
    for m in mods {
        let Some(ref routes_dir) = m.routes_dir else {
            continue;
        };
        if !m.routes_present() {
            continue;
        }
        let subtree = match RouteTree::scan(routes_dir) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "warn: module `{}` (at {}) routes scan failed: {} {}",
                    m.name, m.dir.display(), e.0.code, e.0.message
                );
                continue;
            }
        };
        let prefix = format!("modules/{}/", m.name);
        let mut count = 0usize;
        for mut entry in subtree.entries {
            // Prefix `segments` so the URL is `/modules/<name>/…`. The
            // matcher keys on `segments`, so a module route can never shadow
            // a project route.
            let mut new_segs = vec!["modules".to_string(), m.name.clone()];
            new_segs.append(&mut entry.segments);
            entry.segments = new_segs;
            // Namespace the id too, so log output and manifests distinguish
            // a module route from a project route with the same shape.
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
/// `transform` is wired (`build_js_plugin_hook`), so a module with ONLY
/// `transform` no longer triggers the notice. Any other hook name still
/// does — an honest signal that enabling it in config will not make it
/// fire yet.
pub fn has_unmerged_hooks(mods: &[ResolvedModule]) -> Vec<&ResolvedModule> {
    mods.iter()
        .filter(|m| m.hooks.iter().any(|h| h != "transform"))
        .collect()
}
