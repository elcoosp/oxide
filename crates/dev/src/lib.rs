#![doc = include_str!("../README.md")]

//! oxide-fw-dev — watcher, HMR, overlay, inspector (spec §13).
//!
//! Startup (§13.1): load config (static extract), open persistent cache, start the
//! file watcher, restore route manifest + prebundled deps, start listening — no
//! bundling before the first request. Serving (§13.2): native ESM with `?v=<hash>`
//! cache busting; the server environment reads transformed modules straight from the
//! graph engine (no HTTP hop, D15). HMR protocol (§13.3) below.

use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directories never traversed by the watcher (§5.5): VCS, package store, framework
/// cache and build output are non-source.
const SKIP_DIRS: &[&str] = &[".git", "node_modules", ".oxide", "target"];

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// Watcher with debounce + content-hash confirmation so touch-without-change does
/// nothing (§5.5). Lockfile changes invalidate resolver nodes.
pub struct Watcher {
    pub roots: Vec<PathBuf>,
    pub debounce: Duration,
    /// PERF-11: the OS-native watcher backend. When `Some`, `poll` drains its
    /// event buffer (debounced) and content-hashes only the touched paths,
    /// instead of re-walking + re-hashing the whole tree every tick. `None`
    /// (notify failed to start, or a headless environment) falls back to the
    /// full-walk poll.
    backend: Option<NotifyBackend>,
}

/// PERF-11: a `notify` watcher plus the path buffer its callback fills.
/// The watcher handle is shared so `poll_notify` can add watches for
/// directories that appear while the server is running (§1.3).
struct NotifyBackend {
    /// Adds watches for directories created after boot; also owns the OS
    /// watch for the backend's lifetime.
    watcher: std::sync::Arc<std::sync::Mutex<notify::RecommendedWatcher>>,
    rx: std::sync::mpsc::Receiver<PathBuf>,
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
            backend: None,
        }
    }

    /// §1.3 (boot baseline): snapshot every watched file's content hash into
    /// `seen` BEFORE the event loop's first poll. With the notify backend the
    /// kernel only reports paths that are TOUCHED — but on boot the dev
    /// server itself (workspace-package rebuild, prebundle write-out) and
    /// external processes (editors, sync daemons) touch dozens of
    /// pre-existing files. With an empty `seen`, every one of those looked
    /// like `Created` and fanned out into module updates / route-data
    /// updates / full reloads for files nobody edited — the "boot storm"
    /// that made the first seconds after `oxide dev` nondeterministic for
    /// any client that connected during it. Seeding makes the CURRENT
    /// on-disk state the baseline: a touched-but-unchanged file diffs to no
    /// event; a real edit still diffs to `Modified`; a genuinely new file
    /// still diffs to `Created`.
    ///
    /// Under `node_modules` the seed is deliberately NARROW: only
    /// `package.json` entries (the single node_modules path class whose
    /// events survive the CLI's HMR pre-filter — everything else under
    /// node_modules is dropped there). Hashing the whole dependency tree is
    /// both wasteful and actively harmful: walking `.pnpm`'s store paths
    /// costs seconds on a cold cache, and the watcher loop only starts
    /// AFTER the seed — a slow seed delays change detection for every early
    /// edit. The top-level links (`node_modules/<pkg>`,
    /// `node_modules/@scope/<pkg>`) are hashed THROUGH the symlink so the
    /// inotify watch-add race (which reports those entries as spurious
    /// `Created` on the first post-boot ticks) diffs against reality.
    pub fn seed_seen(&self, seen: &mut std::collections::BTreeSet<(PathBuf, String)>) {
        let mut current: BTreeMap<PathBuf, String> = BTreeMap::new();
        for root in &self.roots {
            walk_root(root, &mut current);
            seed_node_modules_pkg_jsons(root, &mut current);
        }
        seen.extend(current);
    }

    /// PERF-11: a `Watcher` backed by the OS-native `notify` watcher. Falls
    /// back to the poll backend (with a warning) when the watcher cannot be
    /// created — inotify limits, a network filesystem, a headless CI box.
    pub fn with_notify(roots: Vec<PathBuf>) -> Self {
        match Self::try_notify(&roots) {
            Ok(backend) => Self {
                roots,
                debounce: Duration::from_millis(30),
                backend: Some(backend),
            },
            Err(e) => {
                // Not `eprintln!`: the dev loop is a tracing pipeline — an
                // unstructured stderr line bypassed every subscriber/JSON
                // sink (and fired on every headless boot). A warn span keeps
                // the fallback visible without polluting non-TTY output.
                tracing::warn!(error = %e, "notify watcher unavailable; using the poll backend");
                Self::new(roots)
            }
        }
    }

    fn try_notify(roots: &[PathBuf]) -> Result<NotifyBackend> {
        use notify::RecursiveMode;
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.send(p);
                }
            }
        })
        .map_err(|e| crate::err("DEV-NOTIFY", format!("create watcher: {e}")))?;
        let watcher = std::sync::Arc::new(std::sync::Mutex::new(watcher));
        for root in roots {
            // §1.3 (watch budget): a plain `RecursiveMode::Recursive` watch
            // on the app root registers one inotify watch per directory —
            // including every directory under `node_modules` (~10k dirs in
            // a pnpm store) — which blows the kernel's per-user watch cap
            // (`fs.inotify.max_user_watches`, 8192 here) whenever browsers
            // and other dev servers hold watches too. Once the cap is hit,
            // watch registration fails MID-TREE and events for the
            // unregistered subtrees are silently lost — the watcher appears
            // to work for some files and never fires for others, depending
            // on how many watches other processes happened to hold at
            // boot. Prune instead: descend everywhere EXCEPT the pnpm
            // store's bulk, and cap the node_modules descent at the
            // top-level package dirs (whose direct `package.json` children
            // are the §3.3 prebundle-invalidation signal).
            add_pruned_watches(&watcher, root, RecursiveMode::NonRecursive, 0)?;
        }
        Ok(NotifyBackend { watcher, rx })
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
        // PERF-11: with the notify backend, only the touched paths are
        // re-hashed. The callback buffer is drained (debounced), the changed
        // set is content-hashed, and events emitted only for real changes.
        if let Some(backend) = &self.backend {
            return self.poll_notify(backend, seen);
        }
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

    /// PERF-11: drain the notify buffer, hash only the touched paths, and
    /// diff against `seen`. A touched path that does not exist is a removal.
    fn poll_notify(
        &self,
        backend: &NotifyBackend,
        seen: &mut BTreeSet<(PathBuf, String)>,
    ) -> Result<Vec<FileEvent>> {
        // §3.7 latency fix: debounce ONLY when events are queued. Before
        // this, every poll cycle slept `self.debounce` (30ms) even on an
        // idle filesystem — which meant (a) every write paid 30ms on top of
        // the outer poll interval, and (b) every outer cycle blocked the
        // tokio runtime for 30ms regardless of whether anything had
        // happened. Peek first; only sleep when there is something to
        // settle.
        let mut touched: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        while let Ok(p) = backend.rx.try_recv() {
            touched.insert(p);
        }
        if touched.is_empty() {
            return Ok(Vec::new());
        }
        // There ARE events — give the filesystem a moment to settle
        // (editors write in bursts: save, temp, rename), then drain
        // everything queued during the settle.
        std::thread::sleep(self.debounce);
        while let Ok(p) = backend.rx.try_recv() {
            touched.insert(p);
        }
        let mut events = Vec::new();
        for path in touched {
            // Skip editor temp files.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with('~') || name.starts_with('.') && name.ends_with(".swp") {
                continue;
            }
            if path.is_dir() {
                // §1.3 (self-healing watch): the pruned watch tree registers
                // each directory individually, so a directory CREATED after
                // boot is not watched yet. Register it now and scan its
                // direct children — a `mkdir` immediately followed by file
                // writes (the §3.3 prebundle test, a new route folder) would
                // otherwise lose every event inside it to the watch-add
                // race. The children hash-diff against `seen` like any other
                // first sighting: new files emit `Created`.
                let _ = add_watch_dir(&backend.watcher, &path);
                if let Ok(children) = std::fs::read_dir(&path) {
                    for child in children.flatten() {
                        let cp = child.path();
                        if cp.is_dir() {
                            continue;
                        }
                        emit_diff(&cp, seen, &mut events);
                    }
                }
                continue;
            }
            emit_diff(&path, seen, &mut events);
        }
        Ok(events)
    }
}

/// §1.3: directories whose subtree is NEVER watched. `.pnpm` alone holds
/// thousands of directories in a pnpm workspace — the whole point of the
/// pruned watch is to stay far below `fs.inotify.max_user_watches`.
const WATCH_PRUNE_DIRS: &[&str] = &[".git", "target", ".oxide", ".pnpm", ".bin", ".cache"];

/// §1.3: register a single non-recursive watch on `dir`. Best-effort: a
/// registration racing the directory's deletion fails and is ignored.
fn add_watch_dir(
    watcher: &std::sync::Arc<std::sync::Mutex<notify::RecommendedWatcher>>,
    dir: &Path,
) -> Result<()> {
    use notify::{RecursiveMode, Watcher as _};
    let mut w = watcher
        .lock()
        .map_err(|_| crate::err("DEV-NOTIFY", "watcher lock poisoned"))?;
    w.watch(dir, RecursiveMode::NonRecursive)
        .map_err(|e| crate::err("DEV-NOTIFY", format!("watch {}: {e}", dir.display())))
}

/// §1.3 (watch budget): walk `root` and register a NON-recursive inotify
/// watch per directory, pruning the expensive subtree (see
/// [`WATCH_PRUNE_DIRS`]) and capping the `node_modules` descent at its
/// top-level package directories. A plain `RecursiveMode::Recursive` watch
/// spends one watch per directory under node_modules (~10k in a pnpm
/// workspace) and exhausts `fs.inotify.max_user_watches` nondeterministically
/// — the failure mode was a watcher that silently stopped firing for
/// whichever subtrees lost the registration race.
fn add_pruned_watches(
    watcher: &std::sync::Arc<std::sync::Mutex<notify::RecommendedWatcher>>,
    root: &Path,
    _mode: notify::RecursiveMode,
    depth: u8,
) -> Result<()> {
    const NODE_MODULES_MAX_DEPTH: u8 = 2;
    let _ = add_watch_dir(watcher, root);
    let Ok(md) = std::fs::metadata(root) else {
        return Ok(());
    };
    if !md.is_dir() {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if WATCH_PRUNE_DIRS.contains(&name) {
            continue;
        }
        let Ok(emd) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if emd.is_symlink() {
            // Symlinked dirs (pnpm links into the store / workspace
            // packages): the LINK's parent is watched, so creations of the
            // link itself are seen; the target's content is covered by its
            // own watch root (workspace packages) or does not need events
            // (store internals).
            continue;
        }
        if !emd.is_dir() {
            continue;
        }
        let under_nm = root.file_name().and_then(|n| n.to_str()) == Some("node_modules");
        let next_depth = if under_nm { depth + 1 } else { depth };
        if under_nm && next_depth > NODE_MODULES_MAX_DEPTH {
            continue;
        }
        let _ = add_pruned_watches(watcher, &p, _mode, next_depth);
    }
    Ok(())
}

/// §1.3: hash-diff one touched file against `seen`, emitting the event kind
/// the diff implies and updating `seen` in place. Shared by the plain
/// touched-path path and the new-directory child scan.
fn emit_diff(path: &Path, seen: &mut BTreeSet<(PathBuf, String)>, events: &mut Vec<FileEvent>) {
    let old = seen.iter().find(|(p, _)| p == path).cloned();
    match (old, hash_file(path)) {
        (Some((_, old_hash)), Some(new_hash)) if old_hash != new_hash => {
            seen.remove(&(path.to_path_buf(), old_hash.clone()));
            seen.insert((path.to_path_buf(), new_hash));
            events.push(FileEvent::Modified(path.to_path_buf()));
        }
        (None, Some(new_hash)) => {
            seen.insert((path.to_path_buf(), new_hash));
            events.push(FileEvent::Created(path.to_path_buf()));
        }
        (Some((_, old_hash)), None) => {
            seen.remove(&(path.to_path_buf(), old_hash));
            events.push(FileEvent::Removed(path.to_path_buf()));
        }
        _ => {}
    }
}

/// The §13.3 message-schema **v1** update entry: one client-visible change
/// inside an [`HmrMessage::Update`] frame. `kind` is the client's update
/// mechanism (the [`UpdateKind`] wire discriminant the TS client switches
/// on), `accepted_path` is what the client re-imports or swaps (`""` when
/// the mechanism needs only `path`), `timestamp` is the publish-time unix
/// millisecond stamp, and `hash` is the content hash the client can use for
/// cache busting (`""` when the changed file was unreadable or virtual).
///
/// The wire spelling is the spec's camelCase (`acceptedPath`); the §3.2
/// batch and the singular module update dissolve into this one shape — an
/// `Update` frame with N entries IS the batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateEntry {
    /// The §13.3 client update mechanism (`js` = module re-import, `css` =
    /// style swap without reload, `data` = loader re-fetch).
    pub kind: UpdateKind,
    /// The changed file (or, for `data`, the route the loader belongs to).
    pub path: String,
    /// What the client re-imports/swaps — the dev URL of the accepting
    /// module (e.g. `/src/...`, `/@css/...`); empty when `path` alone
    /// drives the update.
    #[serde(rename = "acceptedPath", default)]
    pub accepted_path: String,
    /// Publish time, unix milliseconds.
    pub timestamp: u64,
    /// blake3 content-hash prefix (the watcher's D6 digest, 16 hex) of the
    /// changed file; `""` when unreadable or not file-backed.
    #[serde(default)]
    pub hash: String,
}

impl UpdateEntry {
    /// Stamp an entry at publish time: `unix_millis()` for the timestamp
    /// and [`content_hash`] of `hash_of` for the hash (`None` for entries
    /// that are not file-backed — the tailwind sheet and route patterns).
    #[must_use]
    pub fn stamped(
        kind: UpdateKind,
        path: impl Into<String>,
        accepted_path: impl Into<String>,
        hash_of: Option<&Path>,
    ) -> Self {
        Self {
            kind,
            path: path.into(),
            accepted_path: accepted_path.into(),
            timestamp: unix_millis(),
            hash: hash_of.and_then(content_hash).unwrap_or_default(),
        }
    }
}

/// Unix milliseconds — the schema v1 `timestamp` unit (§13.3).
#[must_use]
pub fn unix_millis() -> u64 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// blake3 content hash (the watcher's D6 digest) under the wire-v1 name:
/// the `hash` field's source. `None` when the file is unreadable (the
/// entry then carries an empty hash).
#[must_use]
pub fn content_hash(path: &Path) -> Option<String> {
    hash_file(path)
}

/// HMR wire frames (§13.3 message schema **v1**): update batches, reloads,
/// errors, pushed loader data and the connected handshake. Update boundaries
/// computed in Rust from the graph ([`boundary::plan_classified`]).
///
/// Every frame serializes behind a `{"v":1}` envelope
/// ([`HmrMessage::SCHEMA_VERSION`]) — `to_json` inserts it, `from_json`/
/// `try_from_json` enforce it. The v0 spellings (`module-update`,
/// `css-update`, `route-data-update`, `batch-module-update`,
/// `server-module-swap`) are retired: they carry no version and are
/// rejected at the v1 gate with [`HmrWireError::Schema`] (the `DEV-HMR-003`
/// story; the TS client keeps a read-compat normalizer for stale tabs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HmrMessage {
    /// §13.3: client modules/stylesheets/data changed. The ESM dev loop
    /// fans the planner's boundaries out as entries: a `js` entry's
    /// `acceptedPath` is the dev URL the client re-imports through the HMR
    /// runtime (React Refresh applies it in place), a `css` entry swaps
    /// its styles without reload, a `data` entry re-fetches the loader.
    /// The §3.2 coalescing publishes one frame with N entries; the batch
    /// of 1 and the singular update are the same shape now.
    Update {
        /// One entry per changed surface, in the planner's batch order.
        updates: Vec<UpdateEntry>,
        /// E01 phase 4: the state-preservation verdict the fan-out's
        /// policy stamped for this plan ([`state::update_frame`] — the
        /// single construction seam). `"keep"` lets the client
        /// snapshot/restore DOM state around the swap (stash across a
        /// fallback reload); `"clear"` carries the reload reason instead.
        /// Absent on pre-substrate frames and decoded as
        /// [`Preservation::Keep`] — the verdict those servers always
        /// meant — so the field is additive, not a schema break.
        #[serde(default)]
        preserve: state::Preservation,
    },
    /// No boundary found on some path: reload the client. The reason is
    /// the spec's spelling and feeds [`full_reload_diagnostic`] for the
    /// §13.4 overlay.
    FullReload {
        reason: String,
    },
    Error {
        diagnostic: Diagnostic,
    },
    /// §13.3: fresh loader data for a route, pushed with the frame. `data`
    /// is the base64-encoded ODF payload (the JSON transport needs text;
    /// the spec's `data` value rides as base64 — see docs/deviations.md).
    /// An empty `data` is not published as `OxData` — a data change without
    /// a payload is an `Update` entry with `kind: 'data'` (the client
    /// re-fetches `/__data`).
    #[serde(rename = "ox:data")]
    OxData {
        /// The route's URL path pattern (the client drops it when the
        /// viewer is on another route — §1.3).
        route: String,
        /// Base64 ODF payload.
        #[serde(default)]
        data: String,
    },
    /// The handshake: the build id so a stale tab can detect a rebuild.
    /// (The v0 field was `build_id`; the v1 schema spells it `build`.)
    Connected {
        build: String,
    },
}

/// The typed failure of [`HmrMessage::try_from_json`]: a parse error or a
/// schema-version mismatch (the `DEV-HMR-003` story — see
/// [`boundary::schema_mismatch_diagnostic`]).
#[derive(Debug, thiserror::Error)]
pub enum HmrWireError {
    /// The frame is not valid JSON (or not a JSON object).
    #[error("not a decodable HMR frame: {0}")]
    Parse(#[from] serde_json::Error),
    /// The frame speaks another schema version (`0` = no version field —
    /// a legacy v0 frame).
    #[error(
        "unsupported HMR schema version: found {found}, this build speaks v{expected} \
         (rebuild the CLI or hard-reload the stale tab)"
    )]
    Schema {
        /// The version the frame declared (`0` when absent).
        found: u32,
        /// The version this build speaks ([`HmrMessage::SCHEMA_VERSION`]).
        expected: u32,
    },
}

impl HmrMessage {
    /// The §13.3 message-schema version this build speaks. `to_json`
    /// stamps it into every frame; `try_from_json` enforces it.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Wire serialization. The failure path is loud, not silent: a message
    /// that cannot serialize is a dev-loop bug, so `to_json` logs the
    /// `serde_json` error through `tracing::error!` and returns the empty
    /// string (the wire never carries a half-frame — the caller sends what
    /// this returns). Callers that must distinguish a serialize failure from
    /// a legitimately empty payload use [`HmrMessage::try_to_json`].
    pub fn to_json(&self) -> String {
        match self.try_to_json() {
            Ok(json) => json,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "HMR message serialization failed; sending an empty frame"
                );
                String::new()
            }
        }
    }
    /// Fallible serialization — the seam for callers that must distinguish
    /// a serialize failure from an empty frame. Emits the schema-v1
    /// envelope: `{"v":1,"type":...,}` (key order is serde_json's; the
    /// client reads fields by name).
    pub fn try_to_json(&self) -> std::result::Result<String, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let serde_json::Value::Object(ref mut map) = value {
            map.insert(
                String::from("v"),
                serde_json::Value::from(Self::SCHEMA_VERSION),
            );
        }
        serde_json::to_string(&value)
    }
    /// Fallible decode — the seam for callers that must distinguish a parse
    /// failure from a schema-version mismatch. Enforces the v1 envelope: a
    /// frame without (or with a foreign) `v` is [`HmrWireError::Schema`],
    /// not a parse error.
    pub fn try_from_json(s: &str) -> std::result::Result<Self, HmrWireError> {
        let value: serde_json::Value = serde_json::from_str(s)?;
        let found = value
            .get("v")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        if found != Self::SCHEMA_VERSION {
            return Err(HmrWireError::Schema {
                found,
                expected: Self::SCHEMA_VERSION,
            });
        }
        Ok(serde_json::from_value(value)?)
    }
    /// Infallible-by-contract decode. The failure path is loud (a frame a
    /// peer sent but we cannot read is a version-skew bug worth a log line):
    /// `from_json` warns through `tracing::warn!` and returns `None`.
    pub fn from_json(s: &str) -> Option<Self> {
        match Self::try_from_json(s) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "HMR frame decode failed; dropping the frame"
                );
                None
            }
        }
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

/// SPEC-14: classify a changed file path into the §13.3 change kind, which
/// selects the HMR behavior (and thus the `HmrMessage` the dev loop emits).
pub fn classify_path(path: &str) -> ChangeKind {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".css") || lower.ends_with(".scss") || lower.ends_with(".sass") {
        ChangeKind::Css
    } else if lower.ends_with(".server.ts") || lower.ends_with(".server.tsx") {
        // A server module: a loader change is data-only; anything else is a
        // swap. We cannot tell the export apart from the path alone, so the
        // safe default is ServerModule (full reload) unless it is a page
        // server module (data).
        if lower.ends_with("page.server.ts") {
            ChangeKind::ServerLoader
        } else {
            ChangeKind::ServerModule
        }
    } else if lower.contains("oxide.config") {
        ChangeKind::ConfigOrRoutes
    } else if lower.contains("routes/") {
        // Only route-structural files change the tree. A CONTENT edit to
        // `page.tsx` / `layout.tsx` / etc. is a component change: the route
        // still exists, only the module body changed. Files like
        // `doc-body.tsx` under `routes/` are shared components and were
        // previously (incorrectly) triggering a full route-tree rescan.
        let fname = lower.rsplit('/').next().unwrap_or("");
        let is_structural = [
            "page.",
            "layout.",
            "error.",
            "not-found.",
            "loading.",
            "template.",
            "route.",
            "socket.",
        ]
        .iter()
        .any(|p| fname.starts_with(p));
        if is_structural {
            ChangeKind::ConfigOrRoutes
        } else {
            ChangeKind::Component
        }
    } else {
        ChangeKind::Component
    }
}

/// Event-aware classification (§3.9). Where [`classify_path`] sees only a
/// path string, this sees the [`FileEvent`] kind, which distinguishes a
/// CONTENT edit to an existing route file from a CREATE/DELETE of one.
///
/// The distinction matters for HMR: a `Modified` edit to
/// `routes/layout.tsx` (or `page.tsx`, `error.tsx`, etc.) only changed the
/// module body — the route still exists, so the client can re-import the
/// module and let React Refresh apply it in place. A `Created` or
/// `Removed` edit changed the route TREE and requires a rescan.
///
/// Prefer this over [`classify_path`] whenever the event kind is in scope.
/// `classify_path` remains for callers that only have a path string
/// (a path alone cannot distinguish the two cases, so it conservatively
/// treats route-file edits as structural).
///
/// `route.*` and `socket.*` are always structural even on `Modified`:
/// those files define endpoints/dispatch tables whose identity, not just
/// body, is semantically load-bearing.
pub fn classify_event(ev: &FileEvent) -> ChangeKind {
    // §3.9: notify (especially FSEvents on macOS) reports some in-place
    // writes as Created, because the write path can go through temp+rename
    // and the kernel delivers a rename-create for a file that already
    // existed. A content edit to `routes/layout.tsx` therefore arrives as
    // either Created (spurious) or Modified (correct) depending on the
    // FS write pattern and the debounce window.
    //
    // Distinguish "actually new" from "spurious Created" via the file's
    // birth time: a file born >1s ago is not new — the event is a body
    // edit for an existing file. Rewrite to Modified before classifying.
    //
    // The 1s threshold is generous by design. A genuine new file has
    // birthtime within the last few ms; anything older is definitionally
    // an existing file being edited.
    let effective_ev: FileEvent = if let FileEvent::Created(path) = ev {
        let is_old = std::fs::metadata(path)
            .and_then(|m| m.created())
            .ok()
            .and_then(|born| std::time::SystemTime::now().duration_since(born).ok())
            .map(|d| d > std::time::Duration::from_millis(1000));
        if is_old.unwrap_or(false) {
            FileEvent::Modified(path.clone())
        } else {
            ev.clone()
        }
    } else {
        ev.clone()
    };
    let ev = &effective_ev;
    let path = match ev {
        FileEvent::Modified(p) | FileEvent::Created(p) | FileEvent::Removed(p) => p,
    };
    let pstr = path.to_string_lossy();
    let lower = pstr.to_ascii_lowercase();

    // Anything `classify_path` already handles unambiguously — CSS,
    // `.server.*` (loader vs. swap depends on the file name only), config
    // — delegates. The event kind is irrelevant for those.
    if lower.ends_with(".css")
        || lower.ends_with(".scss")
        || lower.ends_with(".sass")
        || lower.ends_with(".server.ts")
        || lower.ends_with(".server.tsx")
        || lower.contains("oxide.config")
    {
        return classify_path(&pstr);
    }

    if lower.contains("routes/") {
        let fname = lower.rsplit('/').next().unwrap_or("");

        // Endpoint / dispatch definitions: identity is structural even when
        // only the body was modified.
        let always_structural = ["route.", "socket."];
        if always_structural.iter().any(|p| fname.starts_with(p)) {
            return ChangeKind::ConfigOrRoutes;
        }

        // Route-component files: Modified means in-place, Created/Removed
        // means the tree changed.
        let content_prefix = [
            "page.",
            "layout.",
            "error.",
            "not-found.",
            "loading.",
            "template.",
        ];
        if content_prefix.iter().any(|p| fname.starts_with(p)) {
            return match ev {
                FileEvent::Modified(_) => ChangeKind::Component,
                FileEvent::Created(_) | FileEvent::Removed(_) => ChangeKind::ConfigOrRoutes,
            };
        }

        // Other files under `routes/` (e.g. shared components like
        // `doc-body.tsx`) are pure modules; the event kind does not affect
        // classification.
        return ChangeKind::Component;
    }

    // Everything outside `routes/`: identical to classify_path.
    classify_path(&pstr)
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
        } else if p.is_file()
            && let Some(hash) = hash_file(&p)
        {
            out.insert(p, hash);
        }
    }
}

/// §1.3: seed the dependency tree's `package.json` entries — the ONLY
/// node_modules path class whose events survive the CLI's HMR pre-filter
/// (see `seed_seen`'s doc for why the rest of node_modules stays unseeded).
/// Top-level `node_modules/<pkg>` and `node_modules/@scope/<pkg>` links are
/// hashed THROUGH the symlink; `.pnpm`/`.bin`/`.oxide` are skipped (a real
/// install's new store paths are new files — they diff as `Created` and
/// correctly trigger §3.3 without a baseline).
fn seed_node_modules_pkg_jsons(root: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let nm = root.join("node_modules");
    let Ok(top) = std::fs::read_dir(&nm) else {
        return;
    };
    for entry in top.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name == ".pnpm" || name == ".bin" || name == ".oxide" {
            continue;
        }
        if name.starts_with('@') {
            // A scope directory (possibly a real dir containing package
            // links): seed one level deeper.
            let Ok(pkgs) = std::fs::read_dir(&p) else {
                continue;
            };
            for pkg in pkgs.flatten() {
                seed_pkg_json(&pkg.path(), out);
            }
        } else {
            seed_pkg_json(&p, out);
        }
    }
}

fn seed_pkg_json(pkg_dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let pj = pkg_dir.join("package.json");
    if let Some(hash) = hash_file(&pj) {
        out.insert(pj, hash);
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
    /// §13.3: the build id sent to a newly connected client so it can detect
    /// a stale tab. Set from the client manifest's `build_id`; `"dev"` when
    /// no build exists (a fresh `oxide dev`).
    build_id: String,
    /// §5.6: a monotonically increasing generation bumped on every file
    /// change that invalidates framework modules. In-flight dev dispatches
    /// capture the generation at entry and are aborted if a bump lands
    /// mid-flight — that is the "newer file change cancels in-flight
    /// stale work" side of §5.6. Reset is never needed (u64 wrap is a
    /// practical impossibility).
    stale_generation: std::sync::atomic::AtomicU64,
    /// §5.6: notifications on the generation bump. `wait_stale_generation`
    /// awaits this; the dispatch `tokio::select!`s between the dispatch
    /// future and this future so a mid-flight change aborts.
    stale_notify: tokio::sync::Notify,
}

impl HmrHub {
    pub fn new() -> Self {
        Self::with_build_id("dev")
    }

    /// A hub carrying the current build id. The CLI passes the client
    /// manifest's `build_id` (or `"dev"`).
    pub fn with_build_id(build_id: impl Into<String>) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(128);
        Self {
            tx,
            build_id: build_id.into(),
            stale_generation: std::sync::atomic::AtomicU64::new(0),
            stale_notify: tokio::sync::Notify::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(capacity.max(1));
        Self {
            tx,
            build_id: "dev".to_string(),
            stale_generation: std::sync::atomic::AtomicU64::new(0),
            stale_notify: tokio::sync::Notify::new(),
        }
    }

    /// The build id a `Connected` handshake advertises.
    pub fn build_id(&self) -> &str {
        &self.build_id
    }

    /// A fresh receiver; it sees every message published after this call.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Serialize and broadcast; dropped when no receiver is listening (OK by design).
    pub fn publish(&self, msg: HmrMessage) {
        let _ = self.tx.send(msg.to_json());
    }

    /// The underlying broadcast sender (the server's `/_oxide/hmr` endpoint
    /// subscribes to it). Cloneable, `Send + Sync`.
    pub fn sender(&self) -> std::sync::Arc<tokio::sync::broadcast::Sender<String>> {
        std::sync::Arc::new(self.tx.clone())
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// §5.6: current stale generation. Zero at first boot; each
    /// `bump_stale_generation` monotonic-advances it.
    pub fn stale_generation(&self) -> u64 {
        self.stale_generation
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// §5.6: invalidate in-flight dispatches. Called by `cmd_dev` on every
    /// file change that would invalidate the framework module cache.
    /// Returns the post-bump generation (for the caller's logging, if any).
    pub fn bump_stale_generation(&self) -> u64 {
        let g = self
            .stale_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        // `notify_waiters` wakes every dispatch currently in `select!`.
        // `notify_one` would wake only one — wrong for N parallel requests.
        self.stale_notify.notify_waiters();
        g
    }

    /// §5.6: await the next generation bump. The dev dispatch's
    /// `tokio::select!` races this against the dispatch future so a
    /// mid-flight change aborts instead of serving pre-change output.
    pub async fn wait_stale_generation(&self) {
        self.stale_notify.notified().await;
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InspectorState {
    pub module_graph: Vec<(String, Vec<String>)>,
    pub why: BTreeMap<String, WhyReport>,
    pub routes: Vec<RouteRow>,
    pub cache: Vec<CacheRow>,
    pub waterfalls: Vec<RequestTimingsRow>,
    pub pool: PoolRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhyReport {
    pub module: String,
    pub importers: Vec<String>,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRow {
    pub id: String,
    pub mode: String,
    pub dynamic_reason: Option<String>,
    pub cache_policy: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheRow {
    pub key: String,
    pub layer: String,
    pub tags: Vec<String>,
    pub age_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestTimingsRow {
    pub request_id: String,
    pub waterfall_ms: Vec<(String, f64)>,
    /// §20 / O3: the W3C trace id (`traceparent`'s 32-hex field) when
    /// the inbound request carried one. Correlates this waterfall row
    /// with the same trace in any downstream span consumer (OTLP,
    /// Jaeger, Tempo). `None` when there was no inbound `traceparent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// §20 / §13.4: a bounded, thread-safe recorder of per-request timings.
/// The dispatch closure records every request's `Server-Timing` stages here;
/// the inspector renders the last N as a waterfall (the dev flamegraph).
#[derive(Clone, Default)]
pub struct WaterfallRecorder {
    inner: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<RequestTimingsRow>>>,
}

impl WaterfallRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one request's stages. Keeps the most recent `MAX` entries.
    /// Back-compat shim: existing call sites pass no trace id.
    pub fn record(&self, request_id: &str, waterfall_ms: Vec<(String, f64)>) {
        self.record_staged(request_id, waterfall_ms, None);
    }

    /// Record one request's stages alongside its W3C trace id.
    pub fn record_with_trace(
        &self,
        request_id: &str,
        waterfall_ms: Vec<(String, f64)>,
        trace_id: Option<String>,
    ) {
        self.record_staged(request_id, waterfall_ms, trace_id);
    }

    /// §20 / O3b: merge a request's stage rows by `request_id`.
    ///
    /// A single request can produce stage data from *two* places: the
    /// initial dispatch (middleware + load + render, recorded synchronously)
    /// and the streaming tail (deferred settles, recorded from the spawned
    /// stream-drain task). Both call this method; the second call appends
    /// its stages to the row the first created rather than pushing a new
    /// row. Result: one row per request in `snapshot()`, which is what the
    /// inspector and `oxide_state` MCP tool expect.
    ///
    /// Order-independent: if the settle task's call lands first (only
    /// possible when a request has no defers and drains instantly), it
    /// creates the row; the initial call then appends. Either order
    /// produces a single row.
    ///
    /// `MAX` still bounds the deque — appends to an existing row are free,
    /// pushes evict from the front once the cap is crossed.
    pub fn record_staged(
        &self,
        request_id: &str,
        waterfall_ms: Vec<(String, f64)>,
        trace_id: Option<String>,
    ) {
        const MAX: usize = 50;
        if let Ok(mut q) = self.inner.lock() {
            // Merge into an existing row if one is already present. Search
            // from the back (usually a hit on the last entry); a merge is
            // cheap and does not touch the deque.
            for row in q.iter_mut().rev() {
                if row.request_id == request_id {
                    row.waterfall_ms.extend(waterfall_ms);
                    if row.trace_id.is_none() {
                        row.trace_id = trace_id;
                    }
                    return;
                }
            }
            // No row — create one and enforce the cap.
            q.push_back(RequestTimingsRow {
                request_id: request_id.to_string(),
                waterfall_ms,
                trace_id,
            });
            while q.len() > MAX {
                q.pop_front();
            }
        }
    }

    /// Snapshot for the inspector.
    pub fn snapshot(&self) -> Vec<RequestTimingsRow> {
        self.inner
            .lock()
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for WaterfallRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WaterfallRecorder(..)")
    }
}

impl WaterfallRecorder {
    /// SPEC-19.3 / §19-D: persist the current snapshot to `path` as JSON so
    /// a separate process (the MCP server) can read the waterfalls of the
    /// running dev server. Best-effort — a write failure logs to stderr but
    /// never fails the request that called it.
    pub fn persist_to(&self, path: &std::path::Path) {
        let rows = self.snapshot();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&rows) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

// ---------------------------------------------------------------------------
// §13.4 Inspector — route-table provider
// ---------------------------------------------------------------------------

/// A snapshot source that supplies the route table. Other inspector fields
/// (module graph, cache, pool, waterfalls) stay empty until those subsystems
/// are wired into the dev server — the schema makes that explicit rather than
/// synthesizing placeholder data.
#[derive(Debug, Clone, Default)]
pub struct RouteTableInspector {
    pub routes: Vec<RouteRow>,
    /// §20: the shared per-request timings recorder. `None` renders an empty
    /// waterfall table.
    pub waterfalls: Option<WaterfallRecorder>,
}

impl RouteTableInspector {
    /// Build from a router `table()` result:
    /// `Vec<(pattern, mode, tags)>` (§8.3 typegen output).
    pub fn from_router_table(table: Vec<(String, String, Vec<String>)>) -> Self {
        let routes = table
            .into_iter()
            .map(|(pattern, mode, tags)| RouteRow {
                id: pattern,
                mode,
                dynamic_reason: None,
                cache_policy: if tags.is_empty() {
                    String::new()
                } else {
                    tags.join(",")
                },
            })
            .collect();
        Self {
            routes,
            waterfalls: None,
        }
    }

    /// Like [`Self::from_router_table`] but wired to a shared
    /// [`WaterfallRecorder`], so the inspector's waterfall table reflects the
    /// live per-request timings the dispatch records (§20).
    pub fn from_router_table_with_recorder(
        table: Vec<(String, String, Vec<String>)>,
        recorder: WaterfallRecorder,
    ) -> Self {
        let mut me = Self::from_router_table(table);
        me.waterfalls = Some(recorder);
        me
    }

    /// Produce the inspector state. Module-graph / cache / pool fields stay
    /// empty until those subsystems feed them; the waterfall is populated
    /// from the shared recorder (§20).
    pub fn state(&self) -> InspectorState {
        InspectorState {
            routes: self.routes.clone(),
            waterfalls: self
                .waterfalls
                .as_ref()
                .map(|r| r.snapshot())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// HTML rendering for `/__oxide` (self-contained, no external assets). A
/// route-table view over the state's route list; the raw JSON is available at
/// `/__oxide/state`.
pub fn render_inspector_html(state: &InspectorState) -> String {
    let mut rows = String::new();
    for r in &state.routes {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&r.id),
            html_escape(&r.mode),
            html_escape(r.dynamic_reason.as_deref().unwrap_or("")),
            html_escape(&r.cache_policy),
        ));
    }
    // B-17: render the recorded request waterfalls (the flamegraph view).
    let mut wf_rows = String::new();
    for wf in &state.waterfalls {
        let total: f64 = wf.waterfall_ms.iter().map(|(_, ms)| *ms).sum();
        let mut bars = String::new();
        for (name, ms) in &wf.waterfall_ms {
            // 2px per ms, clamped so a 2s defer cannot blow up the page.
            let width = (ms * 2.0).round().clamp(1.0, 400.0) as u32;
            bars.push_str(&format!(
                "<span class=\"bar\" style=\"width:{}px\" title=\"{}: {:.1}ms\"></span>",
                width,
                html_escape(name),
                ms,
            ));
        }
        wf_rows.push_str(&format!(
            "<tr><td><code>{}</code></td><td class=\"stages\">{}</td><td>{:.1}ms</td></tr>",
            html_escape(&wf.request_id),
            bars,
            total,
        ));
    }
    format!(
        "<!doctype html><html><head><title>Oxide Inspector</title>\
         <style>\
         body{{font:13px/1.5 system-ui,sans-serif;background:#0a0a0a;color:#e6e6e6;margin:0;padding:24px}}\
         h1{{font-size:18px;margin:0 0 16px}}\
         h2{{font-size:14px;text-transform:uppercase;letter-spacing:.1em;color:#888;margin:24px 0 8px}}\
         table{{width:100%;border-collapse:collapse}}\
         th,td{{text-align:left;padding:6px 10px;border-bottom:1px solid #222}}\
         th{{color:#888;font-weight:500}}\
         code{{background:#151515;padding:2px 6px;border-radius:4px}}\
         .bar{{display:inline-block;height:10px;background:#7cb7ff;margin-right:2px;border-radius:2px;vertical-align:middle}}\
         a{{color:#7cb7ff}}\
         </style></head>\
         <body>\
         <h1>Oxide Inspector</h1>\
         <p>Raw JSON: <a href=\"/__oxide/state\">/__oxide/state</a></p>\
         <h2>Routes ({})</h2>\
         <table><thead><tr><th>pattern</th><th>mode</th><th>dynamic reason</th><th>cache policy</th></tr></thead><tbody>{}</tbody></table>\
         <h2>Waterfalls ({})</h2>\
         <table id=\"wf\"><thead><tr><th>request</th><th>stages</th><th>total</th></tr></thead><tbody>{}</tbody></table>\
         </body></html>",
        state.routes.len(),
        rows,
        state.waterfalls.len(),
        wf_rows
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// V2-E01: the HMR boundary planner — the spec-verbatim breadth-first
/// walk-up over the module graph's importers, the self-accepting scanner
/// and the full-reload diagnostics seam (§13.3, §V2-E01).
pub mod boundary;
pub use boundary::{
    Plan, UpdateKind, full_reload_diagnostic, plan, plan_classified, schema_mismatch_diagnostic,
    source_self_accepts,
};

pub mod dev_server;
pub use dev_server::{
    DevCert, DevServerConfig, ProxyMatch, ProxyRule, apply_proxy, generate_dev_cert, local_url,
    network_url, open_browser, render_qr,
};

/// V2-E01 phase 4: the state-preservation substrate — the `preserve`
/// verdict on the v1 `update` frame plus the injectable [`state::StatePolicy`]
/// seam (§V2-E01).
pub mod state;
pub use state::{DefaultStatePolicy, Preservation, StatePolicy};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod esm_module_update_tests {
    use super::*;

    #[test]
    fn wire_v1_entry_accepted_path_deserializes_default() {
        // A v1 entry without `acceptedPath` still parses — the field is
        // optional on the wire (a data entry needs only the route).
        let raw = r#"{"v":1,"type":"update","updates":[{"kind":"js","path":"src/routes/foo/page.tsx","timestamp":0}]}"#;
        let m: HmrMessage = serde_json::from_str(raw).unwrap();
        match m {
            HmrMessage::Update { updates, .. } => {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates[0].path, "src/routes/foo/page.tsx");
                assert_eq!(updates[0].accepted_path, "");
                assert_eq!(updates[0].kind, UpdateKind::Js);
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn wire_v1_entry_roundtrips_with_accepted_path() {
        let m = HmrMessage::Update {
            updates: vec![UpdateEntry {
                kind: UpdateKind::Js,
                path: "src/routes/foo/page.tsx".to_string(),
                accepted_path: "/src/routes/foo/page.tsx".to_string(),
                timestamp: unix_millis(),
                hash: String::new(),
            }],
            preserve: Preservation::default(),
        };
        let j = m.to_json();
        assert!(j.contains("\"v\":1"), "{j}");
        let back = HmrMessage::from_json(&j).unwrap();
        assert_eq!(m, back);
    }
}
