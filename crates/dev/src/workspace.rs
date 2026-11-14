//! The V2-E12 workspace-links substrate (spec §V2-E12).
//!
//! A meta-framework app inside a monorepo imports first-party packages
//! (`packages/ui`) through package-name specifiers; under pnpm those live
//! behind `node_modules` symlinks. The dev loop must resolve them to their
//! REAL trees (`preserveSymlinks: false` in the spec's words — resolve
//! through the link) so that:
//!
//! - the watcher roots gain the linked packages' source dirs, and an edit to
//!   `../packages/ui/src` invalidates the app (§13 — the dev loop never
//!   restarts the process);
//! - the importers-graph harvest can bridge the package edge: a module path
//!   that arrived through the link canonicalizes onto the same real tree the
//!   watcher reports, so graph nodes and watcher events agree and the
//!   boundary planner walks across the edge.
//!
//! The seam is [`WorkspaceLinks`] — discovery is injectable so tests and
//! future resolvers (workspace tools beyond the two manifest shapes) drop in
//! — with [`ManifestWorkspaceLinks`] as the production impl: detect the
//! nearest workspace manifest (`pnpm-workspace.yaml` or `package.json`
//! `workspaces`), expand its package globs (glob-lite: literal segments and
//! one-segment `*`), and resolve only the declared dependencies that ARE
//! workspace packages through `oxc_resolver`. A malformed workspace manifest
//! is a LOUD `DEV-WS-001` refusal naming the manifest and the fix — the
//! house convention; an uninstalled dependency is skipped with a warn (the
//! install has not run yet — fatal would be wrong).
//!
//! Every public item here is allocation-bounded by the workspace size; the
//! discovery runs once per `oxide dev` boot (plus the per-node
//! [`real_path`] probes the graph harvest runs), so the `workspace_links`
//! criterion bench documents the cost.

use crate::{OxideError, Result};
use oxide_fw_diagnostics::Diagnostic;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The registry code for a workspace-discovery refusal (`docs/errors.toml`):
/// a manifest exists but cannot be read or parsed into the two supported
/// shapes. Public so consumers can match on it; the message names the
/// manifest path.
pub const DEV_WS_001: &str = "DEV-WS-001";

/// One linked workspace package the app imports, resolved to its REAL tree.
///
/// `real_root` is the package directory a symlink in `node_modules` points
/// at (never the link itself) — the spec's `preserveSymlinks: false` result.
/// `watch_root` is the directory the watcher owns: the package's `src/`
/// when it has one, else the package root (a package that ships its source
/// at the root is still watched whole).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkedPackage {
    /// The package name as the app's manifest declares it (`@acme/ui`).
    pub name: String,
    /// The real package directory (symlinks resolved).
    pub real_root: PathBuf,
    /// The directory the watcher adds (the source tree inside the package).
    pub watch_root: PathBuf,
}

/// The workspace-links seam: which packages does the app rooted at
/// `app_root` import from the enclosing workspace, and where do they REALLY
/// live? Injectable so tests pin the discovery contract over fakes and a
/// future workspace tool plugs in without touching the watcher.
pub trait WorkspaceLinks: Send + Sync {
    /// Discover the linked packages for the app rooted at `app_root`.
    ///
    /// A non-workspace app yields `Ok(vec![])` — discovery is opt-in by
    /// tree shape, never by configuration. A manifest that exists but does
    /// not parse is a loud error (`DEV-WS-001`), not a silent empty set.
    fn links(&self, app_root: &Path) -> Result<Vec<LinkedPackage>>;
}

/// The production discovery over the two manifest shapes the spec names:
/// `pnpm-workspace.yaml` (`packages:` globs) and `package.json`
/// `workspaces` (npm/yarn classic). Unit struct — all state is on disk.
#[derive(Debug, Default, Clone, Copy)]
pub struct ManifestWorkspaceLinks;

impl WorkspaceLinks for ManifestWorkspaceLinks {
    fn links(&self, app_root: &Path) -> Result<Vec<LinkedPackage>> {
        let Some((ws_root, globs)) = workspace_manifest(app_root)? else {
            tracing::debug!(
                app = %app_root.display(),
                "workspace links: no workspace manifest above the app"
            );
            return Ok(Vec::new());
        };
        let candidates = workspace_candidates(&ws_root, &globs);
        if candidates.is_empty() {
            tracing::debug!(
                manifest = %ws_root.display(),
                "workspace links: the manifest declares no resolvable packages"
            );
            return Ok(Vec::new());
        }
        let app_deps = app_dependencies(app_root)?;
        if app_deps.is_empty() {
            tracing::debug!(
                app = %app_root.display(),
                "workspace links: the app declares no dependencies"
            );
            return Ok(Vec::new());
        }
        // A link = a workspace candidate the app actually depends on, by
        // name. A published package that merely shares a name with a
        // workspace member is not a link; a workspace member the app never
        // imports is not watched (its churn is not this app's invalidation).
        let linked: Vec<&PathBuf> = candidates
            .iter()
            .filter(|dir| candidate_name(dir).is_some_and(|n| app_deps.contains(&n)))
            .collect();
        if linked.is_empty() {
            tracing::debug!(
                app = %app_root.display(),
                "workspace links: no dependency names a workspace package"
            );
            return Ok(Vec::new());
        }
        // One resolver for the whole pass; symlink-resolving defaults (the
        // spec's `preserveSymlinks: false`) report the REAL entry path.
        let resolver = oxc_resolver::Resolver::new(link_resolve_options());
        let mut links: Vec<LinkedPackage> = Vec::new();
        for dir in linked {
            let Some(name) = candidate_name(dir) else {
                continue;
            };
            let Some(entry) = resolve_linked_entry(&resolver, app_root, &name) else {
                // The dependency is declared and belongs to the workspace,
                // but node_modules does not hold it yet. Skip loudly — a
                // fatal here would abort the dev boot because an install
                // has not run.
                tracing::warn!(
                    name = %name,
                    "workspace dependency did not resolve; run the package \
                     manager's install so linked packages resolve under \
                     node_modules"
                );
                continue;
            };
            let Some(real_root) = package_root_of(&entry) else {
                tracing::warn!(
                    name = %name,
                    entry = %entry.display(),
                    "the resolved entry has no package.json ancestor"
                );
                continue;
            };
            let watch_root = if real_root.join("src").is_dir() {
                real_root.join("src")
            } else {
                real_root.clone()
            };
            tracing::debug!(
                name = %name,
                real_root = %real_root.display(),
                watch_root = %watch_root.display(),
                "workspace link resolved"
            );
            links.push(LinkedPackage {
                name,
                real_root,
                watch_root,
            });
        }
        links.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(links)
    }
}

/// The package-edge bridge for the importers-graph harvest: canonicalize a
/// module path that may have arrived through a `node_modules` link onto the
/// REAL tree, so graph nodes and watcher events agree and the boundary
/// planner walks across the package edge. `None` = the path does not exist
/// (it vanished mid-tick — the caller drops the node).
#[must_use]
pub fn real_path(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// Detect the nearest workspace manifest above `app_root` (the SPEC-16 walk
/// the config crate runs for `extends`, narrowed to the two JS shapes).
/// `Ok(None)` = not inside a workspace; `Err` = a manifest exists but does
/// not parse (loud, `DEV-WS-001`).
fn workspace_manifest(app_root: &Path) -> Result<Option<(PathBuf, Vec<String>)>> {
    for dir in app_root.ancestors() {
        let pnpm = dir.join("pnpm-workspace.yaml");
        if pnpm.is_file() {
            return read_pnpm_workspace(&pnpm).map(|globs| Some((dir.to_path_buf(), globs)));
        }
        let pkg = dir.join("package.json");
        if pkg.is_file() {
            let globs = read_pkg_workspaces(&pkg)?;
            if !globs.is_empty() {
                return Ok(Some((dir.to_path_buf(), globs)));
            }
        }
    }
    Ok(None)
}

/// Parse `pnpm-workspace.yaml`'s `packages:` globs without a YAML
/// dependency: the file's spec surface is one key whose value is either a
/// block list (`- 'glob'` lines) or a flow list (`['glob', ...]`). A scalar
/// value or an unterminated flow list is a loud `DEV-WS-001`. An empty or
/// key-less file means "no packages declared" — an empty candidate set,
/// never an error.
fn read_pnpm_workspace(path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ws_err(path, &format!("read the manifest: {e}")))?;
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("packages:") else {
            // Workspace settings live beside `packages:` (pnpm 10+ catalogs,
            // settings blocks); skip unrelated top-level keys.
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            // Block list: `- 'glob'` lines until a non-entry line.
            let mut globs = Vec::new();
            for item in lines {
                let item = item.trim();
                if item.is_empty() || item.starts_with('#') {
                    continue;
                }
                let Some(entry) = item.strip_prefix("- ") else {
                    break;
                };
                globs.push(unquote(entry.trim()));
            }
            return Ok(globs.into_iter().filter(|g| !g.is_empty()).collect());
        }
        if rest.starts_with('[') {
            return match parse_flow_list(rest) {
                Some(globs) if !globs.is_empty() => Ok(globs),
                _ => Err(ws_err(path, "the `packages:` flow list is unterminated")),
            };
        }
        return Err(ws_err(
            path,
            "the `packages:` value is a scalar; the manifest shape is a \
             block list (`packages:` + `- 'glob'` lines) or a flow list \
             (`packages: ['glob', ...]`)",
        ));
    }
    Ok(Vec::new())
}

/// Parse `package.json`'s `workspaces` — a list of globs, or an object with
/// a `packages` list (yarn classic). Absent = no candidate set.
fn read_pkg_workspaces(path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| ws_err(path, &format!("read the manifest: {e}")))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ws_err(path, &format!("parse the manifest: {e}")))?;
    Ok(match value.get("workspaces") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => glob_strings(items),
        Some(obj) => obj
            .get("packages")
            .and_then(serde_json::Value::as_array)
            .map(|items| glob_strings(items))
            .unwrap_or_default(),
    })
}

/// The string entries of a workspace-glob JSON array.
fn glob_strings(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|v| v.as_str().map(std::borrow::ToOwned::to_owned))
        .collect()
}

/// The app's dependency names (`dependencies` ∪ `devDependencies`), sorted
/// for deterministic discovery. A missing manifest = the app declares
/// nothing (still a valid — if odd — dev tree); a malformed one is loud.
fn app_dependencies(app_root: &Path) -> Result<Vec<String>> {
    let path = app_root.join("package.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| ws_err(&path, &format!("read the app manifest: {e}")))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ws_err(&path, &format!("parse the app manifest: {e}")))?;
    let mut names = BTreeSet::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(map) = value.get(key).and_then(serde_json::Value::as_object) {
            names.extend(map.keys().cloned());
        }
    }
    Ok(names.into_iter().collect())
}

/// Expand the workspace globs into candidate package dirs (glob-lite:
/// literal segments and one-segment `*` — the shapes the spec's fixtures
/// and this repo's own `pnpm-workspace.yaml` use). A candidate must be a
/// directory holding a `package.json` with a `name`; broken candidates are
/// skipped with a warn (a broken sibling must not kill the dev boot).
fn workspace_candidates(root: &Path, globs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for pattern in globs {
        for dir in glob_dirs(root, pattern) {
            let pkg = dir.join("package.json");
            if !pkg.is_file() {
                continue;
            }
            match std::fs::read_to_string(&pkg)
                .map_err(|e| e.to_string())
                .and_then(|raw| {
                    serde_json::from_str::<serde_json::Value>(&raw)
                        .map_err(|e| e.to_string())
                        .and_then(|v| {
                            v.get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(std::borrow::ToOwned::to_owned)
                                .ok_or_else(|| "the candidate package.json has no name".into())
                        })
                }) {
                Ok(_) => out.push(dir),
                Err(why) => {
                    tracing::warn!(
                        manifest = %pkg.display(),
                        why = %why,
                        "candidate package.json is unusable; skipped"
                    );
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The `name` of a candidate package dir (its manifest was validated when
/// the candidate set was built).
fn candidate_name(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(std::borrow::ToOwned::to_owned)
}

/// Match one glob pattern under `root`: every segment is a literal or `*`
/// (exactly one path segment). Depth = the pattern's segment count.
fn glob_dirs(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Vec::new();
    }
    let mut current = vec![root.to_path_buf()];
    for seg in &segments {
        let mut next = Vec::new();
        for dir in &current {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in rd.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if *seg == "*" || name == *seg {
                    next.push(entry.path());
                }
            }
        }
        current = next;
    }
    current.sort();
    current
}

/// Resolve one dependency name from the app root through the symlink-
/// resolving defaults (`preserveSymlinks: false`): the returned entry lives
/// in the REAL package tree. `None` = unresolvable (not installed).
fn resolve_linked_entry(
    resolver: &oxc_resolver::Resolver,
    app_root: &Path,
    name: &str,
) -> Option<PathBuf> {
    resolver
        .resolve(app_root, name)
        .ok()
        .map(|ret| ret.full_path())
}

/// Walk up from a resolved entry file to the nearest `package.json`
/// ancestor — the REAL package root the resolver landed in.
fn package_root_of(entry: &Path) -> Option<PathBuf> {
    entry
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("package.json").is_file())
        .map(Path::to_path_buf)
}

/// The resolver options the link pass runs under: the SSR condition set the
/// bundler's resolver uses, TS-aware extensions — resolution only, never
/// bundling.
fn link_resolve_options() -> oxc_resolver::ResolveOptions {
    oxc_resolver::ResolveOptions {
        condition_names: vec![
            "node".to_string(),
            "import".to_string(),
            "module".to_string(),
            "default".to_string(),
        ],
        extensions: vec![
            ".ts".to_string(),
            ".tsx".to_string(),
            ".js".to_string(),
            ".jsx".to_string(),
            ".mjs".to_string(),
        ],
        ..oxc_resolver::ResolveOptions::default()
    }
}

/// Strip one pair of surrounding quotes from a YAML scalar entry.
fn unquote(entry: &str) -> String {
    for q in ["'", "\""] {
        if let Some(inner) = entry.strip_suffix(q).and_then(|s| s.strip_prefix(q)) {
            return inner.to_string();
        }
    }
    entry.to_string()
}

/// Parse a YAML flow list (`['a', 'b']`) into its quoted scalars.
fn parse_flow_list(rest: &str) -> Option<Vec<String>> {
    let inner = rest.strip_prefix('[')?.strip_suffix(']')?;
    Some(
        inner
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(unquote)
            .collect(),
    )
}

/// The `DEV-WS-001` refusal: the message names the manifest, the fix names
/// both manifest shapes and the install remedy.
fn ws_err(path: &Path, why: &str) -> OxideError {
    OxideError(Box::new(
        Diagnostic::error(
            DEV_WS_001,
            format!("workspace discovery failed for {}: {why}", path.display()),
        )
        .with_fix(
            "check the manifest's shape — pnpm-workspace.yaml `packages:` is \
             a list of glob entries (block or flow), package.json \
             `workspaces` is a list of globs — and run the package \
             manager's install so linked packages resolve under node_modules",
        ),
    ))
}
