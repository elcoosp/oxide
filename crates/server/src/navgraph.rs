//! The learned-prefetch substrate (spec v0.2, §V2-F13).
//!
//! The server keeps an aggregate transition graph between route ids —
//! which route the visitor tends to navigate to next — and answers the
//! client's prefetcher at `GET /_oxide/nav?from=<route>` with the top
//! edges as probabilities. Only aggregate counts are stored: edges are
//! `(from, to)` route-id pairs with a weight, never URLs with params, and
//! no user identifier ever reaches the graph (the spec's k-anonymity
//! floor keeps a transition invisible until enough different sessions
//! took it).
//!
//! This module owns the V8-free pieces:
//!
//! - [`NavGraph`]: the `DashMap<(route, route), f64>` weight table with
//!   [`NavGraph::observe`], [`NavGraph::decay`], [`NavGraph::top`],
//!   [`NavGraph::persist`] / [`NavGraph::load`] and
//!   [`NavGraph::spawn_flusher`].
//! - [`NavGraphSettings`]: the half-life (spec default 7 days), the
//!   k-anonymity floor (spec default 20 observations), the flush interval
//!   (spec default hourly) and the total-edge cap that bounds memory
//!   against a client minting unique route pairs.
//! - [`nav_observation`]: the request-path seam — the `(from, to)` pair
//!   the client stamped on a `/__data` navigation via
//!   [`NAV_FROM_HEADER`] / [`NAV_TO_HEADER`], or `None` when the request
//!   is not an observable navigation.
//! - [`nav_response_from_query`]: the pure endpoint renderer — parse the
//!   `?from=` + optional `?limit=` query, rank, emit
//!   `{"from": …, "edges": [{"to": …, "probability": …}]}`.
//!
//! Two deliberate spec deltas (recorded in `docs/deviations.md`):
//! 1. **Route ids ride two headers.** The spec's `observe(from, to)`
//!    contract says "edges are route ids, never URLs with params", but the
//!    `/__data?route=` value is the concrete path (e.g. `/blog/a`), not the
//!    id (e.g. `/blog/[slug]`). The client router therefore stamps the
//!    *target's* route id in [`NAV_TO_HEADER`] alongside the spec's
//!    [`NAV_FROM_HEADER`]; both must be present and sane or nothing is
//!    observed.
//! 2. **Garbage is skipped, not folded.** The vitals beacon folds a
//!    malformed route label into `_other`; a nav edge must not — a folded
//!    `_other` target would accumulate an artificial popular transition.
//!    An unsanitized route id simply observes nothing.
//!
//! The wiring — attaching the graph to the [`Pipeline`] when
//! `prefetch.learned` is on, observing the data plane in the native
//! prefix, and the hourly snapshot flush — lives in `crates/cli`'s
//! `start_server_inner_full` (a handful of lines; the endpoint and the
//! observation arm are the pipeline's own).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashMap;

use crate::OxideRequest;
use crate::OxideResponse;

/// The request header carrying the navigation's source route id
/// (`x-oxide-nav-from`, §V2-F13 step 1).
pub const NAV_FROM_HEADER: &str = "x-oxide-nav-from";

/// The request header carrying the navigation's target route id — the
/// delta that keeps edges route ids instead of param-carrying paths (see
/// the module doc).
pub const NAV_TO_HEADER: &str = "x-oxide-nav-to";

/// The ops-plane endpoint path (`GET /_oxide/nav?from=<route>`).
pub const NAV_ENDPOINT_PATH: &str = "/_oxide/nav";

/// The default edge-count floor below which a transition stays invisible
/// (the spec's k-anonymity default: 20 observations).
pub const DEFAULT_K_ANON: f64 = 20.0;

/// The default half-life in seconds (the spec's 7 days).
pub const DEFAULT_HALF_LIFE_S: f64 = 7.0 * 24.0 * 3600.0;

/// The default snapshot flush interval in seconds (the spec's hourly).
pub const DEFAULT_FLUSH_SECS: u64 = 3600;

/// The default total-edge cap. One observed pair is two route ids plus a
/// weight (~80 bytes); the cap bounds the worst case — a client minting
/// unique pairs — at a few MB, and eviction-free operation for any real
/// site's route-graph shape.
pub const DEFAULT_MAX_EDGES: usize = 50_000;

/// The knobs the spec fixes at one sentence each; the CLI constructs the
/// graph with these defaults (they are not config surface — the spec's
/// `prefetch` section carries only `learned` and `minProbability`).
#[derive(Debug, Clone, PartialEq)]
pub struct NavGraphSettings {
    /// Weight decay half-life, seconds. Every edge's weight halves once
    /// per half-life, so a transition pattern that stopped being taken
    /// fades out instead of fossilizing.
    pub half_life_s: f64,
    /// The k-anonymity floor: an edge observed fewer than this many times
    /// (post-decay) is invisible to `top`. Non-integer on purpose — decay
    /// produces fractional weights.
    pub k_anon: f64,
    /// The snapshot flush interval, seconds (the spec's hourly flush to
    /// `.oxide/nav-graph.json`).
    pub flush_secs: u64,
    /// The total-edge cap; further *new* edges are dropped (existing edges
    /// keep accumulating weight).
    pub max_edges: usize,
}

impl Default for NavGraphSettings {
    fn default() -> Self {
        Self {
            half_life_s: DEFAULT_HALF_LIFE_S,
            k_anon: DEFAULT_K_ANON,
            flush_secs: DEFAULT_FLUSH_SECS,
            max_edges: DEFAULT_MAX_EDGES,
        }
    }
}

/// One ranked edge: the target route id and the probability a visitor on
/// `from` goes there next (the edge weight over the total outgoing weight,
/// after the k-anonymity filter).
#[derive(Debug, Clone, PartialEq)]
pub struct NavEdge {
    /// The target route id (a route-id, never a param-carrying URL).
    pub to: String,
    /// `weight / total`, in `[0, 1]` (f64 end to end — the weights are
    /// f64, so a truncating cast would silently reorder near-ties).
    pub probability: f64,
}

/// The aggregate route-transition graph (§V2-F13 step 2).
///
/// Counts only: an edge is a `(from, to)` route-id pair and its weight.
/// Weights decay exponentially (halved per half-life) so the graph tracks
/// recent visitor behavior; the decay is applied lazily — on `observe`
/// (throttled to once per minute of wall clock), on `top` and before
/// `persist` — instead of by a background timer, which keeps an idle
/// server's graph cost-free and makes `decay(now)` testable with an
/// injected instant.
pub struct NavGraph {
    edges: DashMap<(String, String), f64>,
    settings: NavGraphSettings,
    /// The anchor the last decay ran from (`decay` and the throttled
    /// lazy decays update it; the elapsed time to it drives the factor).
    last_decay: Mutex<Instant>,
}

impl NavGraph {
    /// A fresh graph with the spec's defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::with_settings(NavGraphSettings::default())
    }

    /// A fresh graph with explicit knobs (tests and non-default hosts).
    #[must_use]
    pub fn with_settings(settings: NavGraphSettings) -> Self {
        Self {
            edges: DashMap::new(),
            settings,
            last_decay: Mutex::new(Instant::now()),
        }
    }

    /// The configured knobs.
    #[must_use]
    pub fn settings(&self) -> &NavGraphSettings {
        &self.settings
    }

    /// The number of distinct edges currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Whether no edge is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Record one transition `from → to` (§V2-F13 step 2).
    ///
    /// Both route ids are sanitized (see [`sanitize_edge_route`]); a value
    /// that fails the shape check observes NOTHING — the skip-not-fold
    /// posture the module doc documents. New edges are dropped once the
    /// graph is at [`NavGraphSettings::max_edges`]; existing edges keep
    /// accumulating weight.
    pub fn observe(&self, from: &str, to: &str) {
        let (Some(from), Some(to)) = (sanitize_edge_route(from), sanitize_edge_route(to)) else {
            tracing::debug!(
                from = %from,
                to = %to,
                "nav edge skipped: route id failed the sanitization shape"
            );
            return;
        };
        // Throttled lazy decay: at most once per minute of wall clock —
        // bounded cost on the observe path, and a no-op for a quiet graph.
        if let Ok(mut last) = self.last_decay.lock()
            && last.elapsed() >= Duration::from_secs(60)
        {
            let now = Instant::now();
            self.decay_locked(now, &mut last);
        }
        if self.settings.max_edges > 0 && self.edges.len() >= self.settings.max_edges {
            // A full graph admits only weight on edges it already knows:
            // `entry` on an existing pair takes the occupied branch.
            if let Some(mut e) = self.edges.get_mut(&(from, to)) {
                *e += 1.0;
            } else {
                tracing::debug!(
                    total = self.edges.len(),
                    cap = self.settings.max_edges,
                    "nav graph at edge cap; new edge dropped"
                );
            }
            return;
        }
        *self.edges.entry((from, to)).or_insert(0.0) += 1.0;
    }

    /// Apply the half-life decay at an explicit instant (§V2-F13's
    /// `decay(now)`). `elapsed <= 0` (a `now` before the last anchor)
    /// applies no factor — decay never grows weights.
    pub fn decay(&self, now: Instant) {
        if let Ok(mut last) = self.last_decay.lock() {
            self.decay_locked(now, &mut last);
        }
    }

    /// The locked decay body: the factor off `last`, applied to every
    /// weight, then the anchor moved forward to `now` (a `now` before the
    /// anchor leaves it — decay never re-grows weights it already shrank).
    fn decay_locked(&self, now: Instant, last: &mut Instant) {
        let elapsed = now.saturating_duration_since(*last).as_secs_f64();
        if elapsed > 0.0 {
            let factor = 0.5f64.powf(elapsed / self.settings.half_life_s);
            for mut e in self.edges.iter_mut() {
                *e.value_mut() *= factor;
            }
        }
        if now > *last {
            *last = now;
        }
    }

    /// The top `k` edges out of `from`, ranked by weight, with edges below
    /// the k-anonymity floor hidden and weights normalized to
    /// probabilities over the visible total (§V2-F13's `top`).
    #[must_use]
    pub fn top(&self, from: &str, k: usize) -> Vec<NavEdge> {
        // Rank on the current (decayed) weights; the lazy decay applies
        // real elapsed time since the last anchor.
        self.decay(Instant::now());
        // The floor comparison carries a relative tolerance: the lazy decay
        // runs on every `top`, so a freshly-observed edge at exactly the
        // floor sits at `floor * (1 - 1e-10)` microseconds later. A real
        // decay crossing (half a half-life = x0.707) dwarfs the tolerance;
        // floating-point noise must not flip visibility.
        let floor = self.settings.k_anon - (self.settings.k_anon * 1e-6).max(1e-9);
        let mut v: Vec<(String, f64)> = self
            .edges
            .iter()
            .filter(|e| e.key().0 == from && *e.value() >= floor)
            .map(|e| (e.key().1.clone(), *e.value()))
            .collect();
        // The spec's sort; ties break deterministically by target id so a
        // synthetic replay is stable across runs.
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let total: f64 = v.iter().map(|x| x.1).sum();
        if total <= 0.0 {
            return Vec::new();
        }
        v.truncate(k);
        v.into_iter()
            .map(|(to, w)| NavEdge {
                to,
                probability: w / total,
            })
            .collect()
    }

    /// Write the graph snapshot to `path` — a `{version, edges: {"from":
    /// {"to": weight}}}` JSON document, written atomically (tmp + rename)
    /// so a crash mid-flush never truncates the previous snapshot.
    /// Decay runs first, so the snapshot carries current weights.
    ///
    /// # Errors
    ///
    /// Propagates the filesystem error; the flusher task logs and keeps
    /// the previous snapshot rather than crashing the server.
    pub fn persist(&self, path: &Path) -> std::io::Result<()> {
        self.decay(Instant::now());
        let mut froms: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for e in &self.edges {
            froms
                .entry(e.key().0.clone())
                .or_default()
                .insert(e.key().1.clone(), *e.value());
        }
        let body = serde_json::json!({ "version": 1, "edges": froms });
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(serde_json::to_string(&body).unwrap_or_default().as_bytes())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, path)
    }

    /// Load a snapshot written by [`NavGraph::persist`] into a fresh
    /// graph. A missing file, unparsable JSON, an unexpected version or an
    /// unexpected edges shape yields a fresh graph (the A07
    /// `StatusSidecars::scan` posture: malformed content is never a
    /// serve-start failure). Weights below the k-anonymity floor are
    /// loaded anyway — decay may bring them back above it, and the floor
    /// is enforced at read time.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let g = Self::new();
        g.merge_from(path);
        g
    }

    /// Merge a snapshot INTO this graph (the `load` body). Weights for
    /// edges the graph already holds add up, so a restart's loaded
    /// snapshot and the requests between load and merge both count. Each
    /// malformed edge row is skipped; the return value is the number of
    /// edges merged. See [`NavGraph::load`] for the malformed-file
    /// posture.
    pub fn merge_from(&self, path: &Path) -> usize {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return 0;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            tracing::warn!(path = %path.display(), "nav graph snapshot is not JSON; starting fresh");
            return 0;
        };
        if v.get("version").and_then(serde_json::Value::as_i64) != Some(1) {
            tracing::warn!(path = %path.display(), "nav graph snapshot version mismatch; starting fresh");
            return 0;
        }
        let Some(edges) = v.get("edges").and_then(serde_json::Value::as_object) else {
            tracing::warn!(path = %path.display(), "nav graph snapshot has no edges map; starting fresh");
            return 0;
        };
        let mut merged = 0usize;
        for (from, tos) in edges {
            let Some(tos) = tos.as_object() else { continue };
            for (to, weight) in tos {
                let (Some(from), Some(to), Some(w)) = (
                    sanitize_edge_route(from),
                    sanitize_edge_route(to),
                    weight.as_f64(),
                ) else {
                    continue;
                };
                if w > 0.0 {
                    *self.edges.entry((from, to)).or_insert(0.0) += w;
                    merged += 1;
                }
            }
        }
        if merged > 0 {
            tracing::info!(
                path = %path.display(),
                edges = merged,
                "nav graph snapshot loaded"
            );
        }
        merged
    }

    /// Spawn the periodic snapshot flusher (§V2-F13 step 3's hourly
    /// flush). One snapshot is written immediately — a restart never
    /// loses the graph it just loaded — then every
    /// [`NavGraphSettings::flush_secs`]. A failed write logs a warning and
    /// retries on the next tick; abort the returned handle (the CLI keeps
    /// it for the server's lifetime).
    #[must_use]
    pub fn spawn_flusher(
        self: &Arc<Self>,
        path: std::path::PathBuf,
    ) -> tokio::task::JoinHandle<()> {
        let g = Arc::clone(self);
        let interval = Duration::from_secs(g.settings.flush_secs.max(1));
        tokio::spawn(async move {
            if let Err(e) = g.persist(&path) {
                tracing::warn!(error = %e, "nav graph flush failed");
            }
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // the immediate tick fires now; skip it
            loop {
                tick.tick().await;
                if let Err(e) = g.persist(&path) {
                    tracing::warn!(error = %e, "nav graph flush failed");
                }
            }
        })
    }
}

impl Default for NavGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for NavGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavGraph")
            .field("edges", &self.edges.len())
            .field("settings", &self.settings)
            .field("last_decay", &self.last_decay)
            .finish_non_exhaustive()
    }
}

/// The route-id shape a nav edge may carry: no query, no fragment, no
/// whitespace, at most 128 bytes of `[A-Za-z0-9_/.\-:\[\]]` (the route-id
/// alphabet the router's own ids use, plus the dynamic-segment brackets —
/// `/blog/[slug]` is a legitimate edge endpoint; `/blog/a?tracking=1` is
/// not). Everything else is refused — the skip-not-fold posture: the
/// caller observes nothing rather than normalizing an attacker-controlled
/// value into a valid-looking edge.
#[must_use]
pub fn sanitize_edge_route(raw: &str) -> Option<String> {
    let path = raw.trim();
    if path.is_empty()
        || path.len() > 128
        || !path.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.' | b':' | b'[' | b']')
        })
    {
        return None;
    }
    Some(path.to_string())
}

/// The request-path observation seam: `(from, to)` when `req` is a `GET
/// /__data` carrying both nav route-id headers, both surviving
/// [`sanitize_edge_route`] — `None` otherwise (never answers the request;
/// the pipeline observes and falls through).
#[must_use]
pub fn nav_observation(req: &OxideRequest) -> Option<(String, String)> {
    if !req.method.eq_ignore_ascii_case("GET") {
        return None;
    }
    let path = req.url.split('?').next().unwrap_or("");
    if path != "/__data" {
        return None;
    }
    let from = sanitize_edge_route(req.header(NAV_FROM_HEADER).unwrap_or(""))?;
    let to = sanitize_edge_route(req.header(NAV_TO_HEADER).unwrap_or(""))?;
    Some((from, to))
}

/// The pure `GET /_oxide/nav` renderer: parse `query` for `from` (percent
/// decoded, required — a missing or empty `from` is a 400) and the
/// optional `limit` (a positive integer, default 3, capped at 32 so one
/// query cannot serialize the whole graph), rank, and emit the JSON body.
/// A `200` carries `{"from": …, "edges": [{"to": …, "probability": …}]}`.
#[must_use]
pub fn nav_response_from_query(graph: &NavGraph, query: &str) -> OxideResponse {
    let from_raw = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("from="))
        .map(crate::percent_decode)
        .unwrap_or_default();
    let from = sanitize_edge_route(&from_raw).unwrap_or_default();
    if from.is_empty() {
        return nav_error_response(
            400,
            "missing or empty `from`: the nav endpoint is `GET /_oxide/nav?from=<route>`",
        );
    }
    let limit = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("limit=").map(|v| v.parse::<usize>().ok()))
        .and_then(|parsed| parsed)
        .filter(|l| *l > 0)
        .unwrap_or(3)
        .min(32);
    let edges = graph.top(&from, limit);
    let body = serde_json::json!({
        "from": from,
        "edges": edges
            .iter()
            .map(|e| serde_json::json!({ "to": e.to, "probability": e.probability }))
            .collect::<Vec<_>>(),
    });
    OxideResponse::json(200, body)
}

/// The endpoint's error spelling: a plain-text body with the status (the
/// csp-report sink's rejection shape, minus the diagnostic envelope — the
/// browser's prefetcher does not render it).
fn nav_error_response(status: u16, msg: &str) -> OxideResponse {
    OxideResponse::new(status, Bytes::from(msg.to_string()))
        .with_header("content-type", "text/plain; charset=utf-8")
}
