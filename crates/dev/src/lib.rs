//! oxide-dev — watcher, HMR, overlay, inspector (spec §13).
//!
//! Startup (§13.1): load config (static extract), open persistent cache, start the
//! file watcher, restore route manifest + prebundled deps, start listening — no
//! bundling before the first request. Serving (§13.2): native ESM with `?v=<hash>`
//! cache busting; the server environment reads transformed modules straight from the
//! graph engine (no HTTP hop, D15). HMR protocol (§13.3) below.

use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directories never traversed by the watcher (§5.5): VCS, package store, framework
/// cache and build output are non-source.
const SKIP_DIRS: &[&str] = &[".git", "node_modules", ".oxide", "target"];

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Watcher with debounce + content-hash confirmation so touch-without-change does
/// nothing (§5.5). Lockfile changes invalidate resolver nodes.
pub struct Watcher {
    pub roots: Vec<PathBuf>,
    pub debounce: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileEvent {
    Modified(PathBuf),
    Created(PathBuf),
    Removed(PathBuf),
}

impl Watcher {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            debounce: Duration::from_millis(30),
        }
    }

    /// Poll-based snapshot diffing (notify backend slots in behind this API).
    ///
    /// Semantics (§5.5): every watched file is hashed with blake3 (first 16 hex
    /// chars); an event is only emitted when the **content hash** changed, so
    /// touch-without-change is a no-op. `seen` is updated in place to the current
    /// snapshot. Removed events are listed before Created/Modified; within each
    /// group events are sorted by path. Debounce (§5.5) is a *caller cadence*
    /// concern: `poll` returns immediately and the caller decides the sampling
    /// interval (`self.debounce` documents the intended gap); the debounced notify
    /// loop lives with the dev server runtime.
    pub fn poll(&self, seen: &mut BTreeSet<(PathBuf, String)>) -> Result<Vec<FileEvent>> {
        let mut current: BTreeMap<PathBuf, String> = BTreeMap::new();
        for root in &self.roots {
            walk_root(root, &mut current);
        }

        let mut events = Vec::new();
        for (path, hash) in &current {
            match seen.iter().find(|(p, _)| p == path) {
                Some((_, old)) if old != hash => events.push(FileEvent::Modified(path.clone())),
                Some(_) => {}
                None => events.push(FileEvent::Created(path.clone())),
            }
        }
        for (path, _) in seen.iter() {
            if !current.contains_key(path) {
                events.push(FileEvent::Removed(path.clone()));
            }
        }

        *seen = current.into_iter().collect();

        events.sort_by(|a, b| {
            let rank = |e: &FileEvent| match e {
                FileEvent::Removed(_) => 0u8,
                _ => 1,
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| path_of(a).cmp(path_of(b)))
        });
        Ok(events)
    }
}

/// HMR message kinds (§13.3): module updates, CSS updates, route-data updates,
/// server updates, full-reload and error events. Update boundary computed in Rust
/// from the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HmrMessage {
    ModuleUpdate {
        path: String,
        boundary: String,
        code: String,
    },
    CssUpdate {
        path: String,
        code: String,
    },
    RouteDataUpdate {
        route: String,
        odf_b64: String,
    },
    ServerModuleSwap {
        path: String,
    },
    FullReload {
        reason: String,
    },
    Error {
        diagnostic: Diagnostic,
    },
    Connected {
        build_id: String,
    },
}

impl HmrMessage {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

/// Per-adapter strategy table (§13.3): what happens for each change kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Component,
    Css,
    ServerLoader,
    ServerModule,
    ConfigOrRoutes,
}

pub fn hmr_behavior(kind: ChangeKind) -> HmrBehavior {
    match kind {
        ChangeKind::Component => HmrBehavior::AdapterHmr,
        ChangeKind::Css => HmrBehavior::StyleSwap,
        ChangeKind::ServerLoader => HmrBehavior::ReloadData,
        ChangeKind::ServerModule => HmrBehavior::ServerSwap,
        ChangeKind::ConfigOrRoutes => HmrBehavior::RestartSubsystems,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HmrBehavior {
    /// Adapter HMR preserves state (React Refresh, Vue HMR, svelte-hmr, solid-refresh).
    AdapterHmr,
    /// Style swap, no reload.
    StyleSwap,
    /// Re-run loader, push new ODF data, re-render in place.
    ReloadData,
    /// Swap in live isolates; next request uses new code; client reloads data.
    ServerSwap,
    /// Restart affected subsystems; process restart only if the adapter changes.
    RestartSubsystems,
}

/// Compute the smallest update boundary for a changed file given reverse deps
/// (importers) — walks up to the nearest HMR boundary (§13.3).
///
/// Boundary rule: `*.server.ts` files and CSS are boundaries. Documented
/// simplification (the full graph walk-up lives in the graph engine): with a single
/// importer the walk continues on it; with multiple importers (or none) the changed
/// file itself becomes the boundary — every importer subtree then refreshes against
/// it instead of being re-resolved per-importer.
pub fn update_boundary(path: &str, importers: &BTreeSet<String>) -> String {
    if is_boundary(path) {
        return path.to_string();
    }
    match importers.len() {
        0 => path.to_string(),
        1 => {
            let single = importers.iter().next().expect("len == 1");
            if single == path {
                path.to_string() // self-import / no further info: stop here
            } else {
                update_boundary(single, importers)
            }
        }
        _ => path.to_string(), // multi-importer: own boundary (see doc above)
    }
}

fn is_boundary(path: &str) -> bool {
    path.ends_with(".server.ts") || path.ends_with(".css")
}

fn path_of(e: &FileEvent) -> &Path {
    match e {
        FileEvent::Modified(p) | FileEvent::Created(p) | FileEvent::Removed(p) => p,
    }
}

fn walk_root(root: &Path, out: &mut BTreeMap<PathBuf, String>) {
    if root.is_file() {
        if let Some(hash) = hash_file(root) {
            out.insert(root.to_path_buf(), hash);
        }
        return;
    }
    if !root.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return; // unreadable root: skip silently, next poll may recover
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if SKIP_DIRS.contains(&name) {
                continue;
            }
            walk_root(&p, out);
        } else if p.is_file() {
            if let Some(hash) = hash_file(&p) {
                out.insert(p, hash);
            }
        }
    }
}

/// blake3 content hash (D6), first 16 hex chars. Unreadable files are skipped (they
/// reappear as Created once readable again).
fn hash_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let hex = blake3::hash(&bytes).to_hex().to_string();
    Some(hex[..16].to_string())
}

/// Broadcast hub for HMR JSON messages (§13.3): the dev server publishes
/// [`HmrMessage`]s; each browser websocket client holds a `subscribe()` receiver.
/// Publishing with zero subscribers is a no-op (broadcast semantics).
pub struct HmrHub {
    tx: tokio::sync::broadcast::Sender<String>,
}

impl HmrHub {
    pub fn new() -> Self {
        Self::with_capacity(128)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// A fresh receiver; it sees every message published after this call.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Serialize and broadcast; dropped when no receiver is listening (OK by design).
    pub fn publish(&self, msg: HmrMessage) {
        let _ = self.tx.send(msg.to_json());
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for HmrHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Error overlay payload: miette-style diagnostics serialized as JSON — code frames,
/// source-mapped stacks, hints with fixes (§13.4). Drawn by a small overlay in dev.
pub fn overlay_payload(diag: &Diagnostic) -> String {
    diag.to_json()
}

/// `/__oxide` inspector data model (§13.4): module graph, `oxide why` explanations,
/// route table with inferred modes, cache entries + tags, per-request waterfalls,
/// isolate pool state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InspectorState {
    pub module_graph: Vec<(String, Vec<String>)>,
    pub why: BTreeMap<String, WhyReport>,
    pub routes: Vec<RouteRow>,
    pub cache: Vec<CacheRow>,
    pub waterfalls: Vec<RequestTimingsRow>,
    pub pool: PoolRow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhyReport {
    pub module: String,
    pub importers: Vec<String>,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRow {
    pub id: String,
    pub mode: String,
    pub dynamic_reason: Option<String>,
    pub cache_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRow {
    pub key: String,
    pub layer: String,
    pub tags: Vec<String>,
    pub age_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestTimingsRow {
    pub request_id: String,
    pub waterfall_ms: Vec<(String, f64)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolRow {
    pub workers: usize,
    pub in_flight: u64,
    pub recycles: u64,
}

/// Dev server features (§13.5): proxy rules, env reload, network URL display,
/// `dev.bundle` full-bundle mode flag. Kept as typed plumbing for the CLI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevOptions {
    pub bundle: bool,
    pub https: bool,
    pub proxy: std::collections::BTreeMap<String, String>,
    pub open_browser: bool,
    pub qr_code: bool,
}

/// Prebundle decision (§6.5 dev dependency prebundling): cache by lockfile hash.
pub fn prebundle_key(lockfile_hash: &str, dep: &str) -> String {
    format!("prebundle:{lockfile_hash}:{dep}")
}

#[cfg(test)]
mod tests;
