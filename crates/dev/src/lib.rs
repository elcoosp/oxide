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
struct NotifyBackend {
    /// Held to keep the OS watch alive for the `Watcher`'s lifetime.
    _watcher: notify::RecommendedWatcher,
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
                eprintln!("warn: notify watcher unavailable ({e}); using the poll backend");
                Self::new(roots)
            }
        }
    }

    fn try_notify(roots: &[PathBuf]) -> Result<NotifyBackend> {
        use notify::{RecursiveMode, Watcher as _};
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.send(p);
                }
            }
        })
        .map_err(|e| crate::err("DEV-NOTIFY", format!("create watcher: {e}")))?;
        for root in roots {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| crate::err("DEV-NOTIFY", format!("watch {}: {e}", root.display())))?;
        }
        Ok(NotifyBackend {
            _watcher: watcher,
            rx,
        })
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
        // Debounce: give the filesystem a moment to settle, then drain
        // everything queued (editors write in bursts — save, temp, rename).
        std::thread::sleep(self.debounce);
        let mut touched: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
        while let Ok(p) = backend.rx.try_recv() {
            touched.insert(p);
        }
        if touched.is_empty() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for path in touched {
            // Skip editor temp files / directories.
            if path.is_dir() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with('~') || name.starts_with('.') && name.ends_with(".swp") {
                continue;
            }
            let old = seen.iter().find(|(p, _)| p == &path).cloned();
            match (old, hash_file(&path)) {
                (Some((_, old_hash)), Some(new_hash)) if old_hash != new_hash => {
                    seen.remove(&(path.clone(), old_hash.clone()));
                    seen.insert((path.clone(), new_hash));
                    events.push(FileEvent::Modified(path));
                }
                (None, Some(new_hash)) => {
                    seen.insert((path.clone(), new_hash));
                    events.push(FileEvent::Created(path));
                }
                (Some((_, old_hash)), None) => {
                    seen.remove(&(path.clone(), old_hash));
                    events.push(FileEvent::Removed(path));
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

/// HMR message kinds (§13.3): module updates, CSS updates, route-data updates,
/// server updates, full-reload and error events. Update boundary computed in Rust
/// from the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HmrMessage {
    /// §13.3: a client module changed. v0.1 does a reload — the adapter HMR
    /// hook (`onModuleUpdate`) is wired but the `code`/`boundary` payload
    /// fields were always empty, so they are gone. Populating them (real
    /// module-swap HMR) is a post-v0.1 track.
    ModuleUpdate {
        path: String,
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
    } else if lower.contains("oxide.config") || lower.contains("routes/") {
        ChangeKind::ConfigOrRoutes
    } else {
        ChangeKind::Component
    }
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
        } else if p.is_file()
            && let Some(hash) = hash_file(&p)
        {
            out.insert(p, hash);
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

pub mod dev_server;
pub use dev_server::{
    DevCert, DevServerConfig, ProxyMatch, ProxyRule, apply_proxy, generate_dev_cert, local_url,
    network_url, open_browser, render_qr,
};

#[cfg(test)]
mod tests;
