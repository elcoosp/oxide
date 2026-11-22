//! §15-E load side: merge `oxide.module` packages into the build.
//!
//! The `add` command records a module name to `.oxide/modules.json` and
//! discovers an `oxide.module` field in the package's `package.json`. This
//! module reads those names back, resolves each package, merges its
//! declared **routes** into the route tree the build scans, surfaces its
//! **transform** hook into the JS plugin chain, merges its **types** into
//! `.oxide/types/`, merges its **configDefaults** under `modules.<name>`,
//! enforces its **adapters** declaration with a loud refusal, and feeds its
//! **middlewares** / **serverFns** through the existing middleware chain
//! and server-fn discovery — all through the EXISTING substrates, never a
//! parallel pipeline.
//!
//! Module contract (V2-E06, the v1 shape the TS `defineModule` in
//! `packages/core/src/module.ts` emits):
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
//!       "configDefaults": { "images": { "remotePatterns": [] } },
//!       "middlewares": [{ "file": "./middleware.ts" }],
//!       "types": [{ "name": "mod", "dts": "declare global {}" }],
//!       "serverFns": [{ "file": "./server/track.ts" }],
//!       "config": { "mod": { "flag": true } }
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
//!
//! Adapters (§V2-E06): the loader enforces the declaration against the
//! project's configured adapter. A mismatch raises `MOD-ADAPTER-002`
//! (`OxideError::with_fix`) registered in `docs/errors.toml`, naming the
//! two lines to paste. The same check fires at `oxide add` time so a
//! mismatch is reported before the install round-trip finishes.
//!
//! Types (§V2-E06): each entry is written to `.oxide/types/<name>.d.ts`
//! (the directory the project's `tsconfig` already includes via the oxide
//! scaffold). A merge is idempotent: the second write replaces the first.
//!
//! configDefaults (§V2-E06): merged under `modules.<name>` in the resolved
//! config and added to the JSON Schema (so editor tooling accepts the key).
//!
//! middlewares (§V2-E06): each entry feeds the existing middleware chain —
//! the route tree's middleware index gains an entry keyed by the module's
//! own `/modules/<name>/` prefix (or the `scope` the module declares), so
//! `RouteTree::middleware_files_for` picks it up exactly like a project
//! middleware.
//!
//! serverFns (§V2-E06): each entry feeds the existing server-fn discovery
//! walk — the file is resolved to an absolute path and appended to the
//! build's server-fn scan list, so the module's server exports get the
//! same `/_oxide/fn/<id>` treatment.

use oxide_fw_config::OxideConfig;
use oxide_fw_diagnostics::OxideError;
use oxide_fw_router::RouteTree;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A middleware file the module contributes (§V2-E06). Mirrors
/// `oxide_fw_plugin::ModuleMiddlewareEntry`.
#[derive(Debug, Clone, Default)]
pub struct ModuleMiddleware {
    pub file: PathBuf,
    pub scope: Option<String>,
}

/// A type declaration the module ships (§V2-E06). Mirrors
/// `oxide_fw_plugin::ModuleTypeEntry`.
#[derive(Debug, Clone, Default)]
pub struct ModuleType {
    pub name: String,
    pub dts: String,
}

/// A server-function file the module exposes (§V2-E06). Mirrors
/// `oxide_fw_plugin::ModuleServerFnEntry`.
#[derive(Debug, Clone, Default)]
pub struct ModuleServerFn {
    pub file: PathBuf,
}

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
    /// §V2-E06: middleware entries the module contributes.
    pub middlewares: Vec<ModuleMiddleware>,
    /// §V2-E06: type declarations the module ships.
    pub types: Vec<ModuleType>,
    /// §V2-E06: server-function files the module exposes.
    pub server_fns: Vec<ModuleServerFn>,
    /// §V2-E06: a config patch the module applies at resolved-config time.
    pub config: Option<serde_json::Value>,
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
    /// no restriction. Enforced against the project adapter at load and
    /// `add` time (`MOD-ADAPTER-002`).
    pub adapters: Vec<String>,
    /// A `.d.ts` the module ships. Reported but not merged — wiring it into
    /// `.oxide/types/` is a documented follow-up.
    pub typegen: Option<PathBuf>,
    /// Default config values keyed by config path. Merged under
    /// `modules.<name>` (the JSON Schema entry is added too).
    pub config_defaults: Option<serde_json::Value>,
    /// §V2-E06: middleware entries the module contributes (absolute paths).
    pub middlewares: Vec<ModuleMiddleware>,
    /// §V2-E06: type declarations the module ships (name + dts body).
    pub types: Vec<ModuleType>,
    /// §V2-E06: server-function files the module exposes (absolute paths).
    pub server_fns: Vec<ModuleServerFn>,
    /// §V2-E06: a config patch the module applies at resolved-config time.
    pub config: Option<serde_json::Value>,
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
    if let Ok(bytes) = std::fs::read(&modules_json)
        && let Ok(arr) = serde_json::from_slice::<Vec<String>>(&bytes)
    {
        for n in arr {
            names.insert(n);
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

    let middlewares = module
        .get("middlewares")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let file = x.get("file")?.as_str()?.to_string();
                    let scope = x.get("scope").and_then(|s| s.as_str()).map(str::to_string);
                    Some(ModuleMiddleware {
                        file: PathBuf::from(file),
                        scope,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let types = module
        .get("types")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let name = x.get("name")?.as_str()?.to_string();
                    let dts = x.get("dts")?.as_str()?.to_string();
                    Some(ModuleType { name, dts })
                })
                .collect()
        })
        .unwrap_or_default();

    let server_fns = module
        .get("serverFns")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let file = x.get("file")?.as_str()?.to_string();
                    Some(ModuleServerFn {
                        file: PathBuf::from(file),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

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
        middlewares,
        types,
        server_fns,
        config: module.get("config").cloned(),
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
    // §V2-E06: resolve middleware/server-fn files to absolute paths under
    // the package dir; the loader feeds them through the existing chains.
    let middlewares = c
        .middlewares
        .into_iter()
        .map(|m| ModuleMiddleware {
            file: pkg_dir.join(&m.file),
            scope: m.scope,
        })
        .collect();
    let server_fns = c
        .server_fns
        .into_iter()
        .map(|s| ModuleServerFn {
            file: pkg_dir.join(&s.file),
        })
        .collect();
    Some(ResolvedModule {
        name,
        dir: pkg_dir.to_path_buf(),
        routes_dir,
        entry,
        hooks: c.hooks,
        adapters: c.adapters,
        typegen,
        config_defaults: c.config_defaults,
        middlewares,
        types: c.types,
        server_fns,
        config: c.config,
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
                    m.name,
                    m.dir.display(),
                    e.0.code,
                    e.0.message
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

/// §V2-E06: enforce the module's `adapters` declaration against the
/// project's configured adapter. A mismatch raises `MOD-ADAPTER-002`
/// (`OxideError::with_fix`) registered in `docs/errors.toml`, naming the
/// two lines to paste. An empty `adapters` array supports every adapter
/// (no refusal). The same check fires at `oxide add` time so a mismatch
/// is reported before the install round-trip finishes.
///
/// # Errors
///
/// `MOD-ADAPTER-002` when the module declares `adapters` and the project
/// adapter is not among them.
pub fn enforce_adapters(mods: &[ResolvedModule], project_adapter: &str) -> Result<(), OxideError> {
    for m in mods {
        if m.adapters.is_empty() {
            continue;
        }
        if !m.adapters.iter().any(|a| a == project_adapter) {
            return Err(OxideError::from(
                oxide_fw_diagnostics::Diagnostic::error(
                    "MOD-ADAPTER-002",
                    format!(
                        "module `{}` declares adapters [{}] but the project adapter is `{}` (§V2-E06)",
                        m.name,
                        m.adapters.join(", "),
                        project_adapter,
                    ),
                )
                .with_fix(format!(
                    "add `{project_adapter}` to the module's `adapters` declaration, or\n  remove the module from `modules: []` in oxide.config.ts"
                )),
            ));
        }
    }
    Ok(())
}

/// §V2-E06: merge each module's `types` (declared `.d.ts` bodies) into
/// `.oxide/types/<name>.d.ts`. Idempotent: a second write replaces the
/// first (the loader is the only writer under that path). Returns the
/// number of files written. A module's `typegen` file (the legacy
/// `typegen` field) is copied as `<name>.d.ts` when no `types` entries
/// exist — the same merge, retrofitted to the older contract.
pub fn merge_types(root: &Path, mods: &[ResolvedModule]) -> usize {
    let types_dir = root.join(".oxide/types");
    let _ = std::fs::create_dir_all(&types_dir);
    let mut count = 0usize;
    for m in mods {
        if m.types.is_empty() {
            // Fall back to the legacy `typegen` file (a single .d.ts the
            // module ships as a file path rather than an inline body).
            if let Some(tg) = &m.typegen
                && let Ok(body) = std::fs::read_to_string(tg)
            {
                let out = types_dir.join(format!("{}.d.ts", sanitize_name(&m.name)));
                if std::fs::write(&out, &body).is_ok() {
                    count += 1;
                }
            }
            continue;
        }
        for t in &m.types {
            let out = types_dir.join(format!("{}.d.ts", sanitize_name(&t.name)));
            if std::fs::write(&out, &t.dts).is_ok() {
                count += 1;
            }
        }
    }
    count
}

/// §V2-E06: collect each module's server-function files (absolute paths)
/// so the build's server-fn discovery walk picks them up alongside the
/// project's own `*.server.ts` files. The paths are filtered to existing
/// files — a missing module server-fn is reported as a warning at build
/// time, not a silent skip.
pub fn collect_server_fns(mods: &[ResolvedModule]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for m in mods {
        for s in &m.server_fns {
            if s.file.is_file() {
                out.push(s.file.clone());
            } else {
                eprintln!(
                    "warn: module `{}` declares server-fn `{}` which does not exist; skipping",
                    m.name,
                    s.file.display()
                );
            }
        }
    }
    out
}

/// §V2-E06: collect each module's middleware contributions as
/// `(scope, file)` pairs the route tree merges via its middleware index.
/// The `scope` defaults to the module's own `/modules/<name>/` prefix so
/// a module middleware only wraps the module's own routes unless the
/// module explicitly opts into a broader scope.
pub fn collect_middlewares(mods: &[ResolvedModule]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for m in mods {
        for mw in &m.middlewares {
            let scope = mw
                .scope
                .clone()
                .unwrap_or_else(|| format!("/modules/{}/", m.name));
            out.push((scope, mw.file.clone()));
        }
    }
    out
}

/// §V2-E06: merge each module's `configDefaults` into the typed
/// `OxideConfig.modules_config` map under the sanitized module name. The
/// merge is a deep object merge (arrays replace, objects recurse). The
/// build reads a module's defaults from `cfg.modules_config` (the JSON
/// Schema entry is in `crates/config/src/schema.rs`).
pub fn merge_config_defaults(cfg: &mut OxideConfig, mods: &[ResolvedModule]) {
    for m in mods {
        let Some(defaults) = &m.config_defaults else {
            continue;
        };
        let key = sanitize_name(&m.name);
        let existing = cfg
            .modules_config
            .remove(&key)
            .unwrap_or(serde_json::Value::Null);
        cfg.modules_config
            .insert(key, deep_merge(existing, defaults.clone()));
    }
}

/// §V2-E06: the manifest the loader produces from a `ResolvedModule` —
/// the bridge from the on-disk `oxide.module` contract to the in-memory
/// `oxide_fw_plugin::ModuleManifest`. One contract: the fields mirror
/// `ModuleManifest` so the plugin registry and the loader agree on the
/// shape (the loader writes the manifest, the registry reads it).
pub fn to_manifest(m: &ResolvedModule) -> oxide_fw_plugin::ModuleManifest {
    use oxide_fw_plugin::{ModuleMiddlewareEntry, ModuleServerFnEntry, ModuleTypeEntry};
    oxide_fw_plugin::ModuleManifest {
        name: m.name.clone(),
        adapters: m.adapters.clone(),
        config_defaults: m
            .config_defaults
            .as_ref()
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            })
            .unwrap_or_default(),
        plugins: Vec::new(),
        routes: m
            .routes_dir
            .as_ref()
            .map(|_| Vec::new())
            .unwrap_or_default(),
        middlewares: m
            .middlewares
            .iter()
            .map(|mw| ModuleMiddlewareEntry {
                file: mw.file.to_string_lossy().into_owned(),
                scope: mw.scope.clone(),
            })
            .collect(),
        types: m
            .types
            .iter()
            .map(|t| ModuleTypeEntry {
                name: t.name.clone(),
                dts: t.dts.clone(),
            })
            .collect(),
        server_fns: m
            .server_fns
            .iter()
            .map(|s| ModuleServerFnEntry {
                file: s.file.to_string_lossy().into_owned(),
            })
            .collect(),
        config: m.config.clone().unwrap_or(serde_json::Value::Null),
        hooks: m.hooks.clone(),
    }
}

/// Sanitize a module name into a filesystem-safe identifier (drops the
/// `@scope/` prefix and replaces `/` with `-`).
fn sanitize_name(name: &str) -> String {
    let trimmed = name.strip_prefix('@').unwrap_or(name);
    trimmed.replace('/', "-")
}

/// Deep-merge two JSON values: objects recurse, arrays replace, scalars
/// from `b` win.
fn deep_merge(a: serde_json::Value, b: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(mut a), Value::Object(b)) => {
            for (k, v) in b {
                let merged = deep_merge(a.remove(&k).unwrap_or(Value::Null), v);
                a.insert(k, merged);
            }
            Value::Object(a)
        }
        (_, b) => b,
    }
}
