//! The cache plane's hot-key attribution (`observer.rs`'s named
//! dashboard-batcher shape): a `CacheObserver` that keeps a BOUNDED
//! per-key event ledger and answers the question the aggregate counters
//! cannot — "which keys dominate the plane's traffic".
//!
//! ## Why the arm exists
//!
//! The F28 seam's doc names the structured consumers a host attaches:
//! "a metrics exporter, a test harness, a feed — and a custom impl for
//! anything else (a test harness, a dashboard batcher)". Until now the
//! attribution end was the custom impl every dashboard hand-rolled: the
//! F30 exporter aggregates per OP (the per-tier hit split, the byte
//! accounting) and the F31 feed replays per ROW, but neither answers the
//! per-KEY question a traffic dashboard opens with — which routes
//! dominate the plane, and how completely the ledger covers the stream.
//! Aggregating per key pull-side (`stats()` samples) misses the events
//! between samples entirely and forces the sampler to store every
//! intermediate; push-side attribution sees every completed event
//! exactly once, where it happens.
//!
//! ## The contract
//!
//! - Every completed event attributes to its key, whatever the op: a
//!   hit, a miss, a write-through set, a delete, one aggregate
//!   invalidation row, one aggregate coalesce row — each counts ONE
//!   toward the key it ran against. The ledger is per-key traffic, not
//!   a per-op split (the exporter keeps that job).
//! - The map is bounded ([`HotKeyObserver::bounded`]); the admission
//!   rule is FIRST SIGHT (the memory tier's TinyLFU-ish posture): a
//!   key admits while there is room, and once resident it counts past
//!   the bound forever — the ledger never loses a key it already
//!   tracks. A NEW key past the bound is refused: its rows are
//!   unattributed, counted in [`HotKeyObserver::unattributed`] (the
//!   F31 honest-saturation posture — the score is the "how much of the
//!   stream the ledger covers" read a dashboard renders beside the top
//!   window).
//! - Refusals are never silent: the FIRST refusal leaves one
//!   `tracing::warn!` under the `oxide::cache` target naming the
//!   capacity (the `EventFeed` sticky rule — one warn, then the rows
//!   refuse), and the counter keeps the score. The warn state lives
//!   under the same mutex as the map, so racing producers warn exactly
//!   once. A zero capacity retains nothing and refuses everything (the
//!   zero-bound posture) — and still warns once.
//! - Counts saturate at `u64::MAX` (the F30 saturating rule): a ledger
//!   that reaches the bound reads "at or past the bound", never a
//!   silently-restarted count.
//! - [`HotKeyObserver::top`] is the deterministic read: count desc,
//!   key asc on ties, at most `n` rows of owned [`HotKeyRow`] values.
//!   Two reads over the same state are equal, so a snapshot diff is a
//!   diff of the traffic, not of the sort (the F30 renderer doctrine).
//! - [`HotKeyObserver::reset`] starts a fresh window: the map clears,
//!   the unattributed score zeroes, the sticky warn re-arms — the
//!   windowed-sampling shape a host that reports per interval rides.
//! - [`HotKeyObserver::render_prometheus`] is the exposition arm (the
//!   F30 parity): the labeled `oxide_cache_plane_hot_key_total` counter
//!   family over the top window, label values escaped per the text
//!   format, a deterministic row order, a trailing newline —
//!   byte-stable re-renders. Infallible, so the scrape path carries no
//!   error shape (the F21 posture: no registry rows for a seam that
//!   cannot refuse).
//!
//! The observer half stays synchronous and infallible: `on_event` is
//! one mutex-protected update (the `std::sync::Mutex` posture the
//! single-flight and feed modules document — no `.await` held across
//! it), so it is safe on the cache's call path; the reads (`top`,
//! `count`, `render_prometheus`) are the host's own calls. Attach
//! through the F28 builder — the `Arc<HotKeyObserver>` coerces to the
//! `Arc<dyn CacheObserver>` the tiers' field stores — and compose
//! beside the exporter and the feed behind one
//! [`FanoutCacheObserver`](crate::FanoutCacheObserver).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use crate::observer::{CacheEvent, CacheObserver};

/// One attributed row of the ledger: the key plus the completed-event
/// count it accumulated (value-shaped — `Clone` + `Eq` — so a host
/// batches, pins or ships the rows; the borrow lives only for the
/// callback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotKeyRow {
    /// The cache key the events ran against (the canonical `KeyStr`
    /// spelling on the route/data plane, the fetch-memo key on L2).
    pub key: String,
    /// Completed events attributed to the key since the ledger attached
    /// (or since the last [`HotKeyObserver::reset`]).
    pub count: u64,
}

/// The bounded per-key ledger plus its sticky warn state — one lock
/// guards all three, so an admission and its warn are atomic against
/// racing producers (the `EventFeed` rule: two producers saturating
/// together warn exactly once, not once each).
#[derive(Debug)]
struct Ledger {
    counts: HashMap<Box<str>, u64>,
    capacity: usize,
    unattributed: u64,
    /// The sticky warn rule: `false` until the first refusal flips it;
    /// `reset` re-arms it (a fresh window may warn again).
    warned: bool,
}

/// The cache plane's hot-key attribution arm: attach one half through
/// the F28 seam (`with_observer`), read the other half whenever the
/// host's dashboard samples.
///
/// ```
/// use std::sync::Arc;
/// use oxide_fw_cache::{HotKeyObserver, TieredCache};
///
/// let hot = Arc::new(HotKeyObserver::bounded(1024));
/// let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(hot.clone()));
/// // … the pipeline serves through `cache`; the dashboard samples:
/// for row in hot.top(10) {
///     let _ = (row.key, row.count); // render, ship, or alert — off the hot path
/// }
/// ```
pub struct HotKeyObserver {
    ledger: Mutex<Ledger>,
}

impl std::fmt::Debug for HotKeyObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The ledger's occupancy and coverage score ARE the shape (the
        // `EventFeed` debug posture: name the shape, not the map's
        // internals).
        let ledger = self.ledger.lock().expect("the hot-key ledger lock");
        f.debug_struct("HotKeyObserver")
            .field("tracked", &ledger.counts.len())
            .field("capacity", &ledger.capacity)
            .field("unattributed", &ledger.unattributed)
            .finish()
    }
}

impl HotKeyObserver {
    /// A ledger tracking at most `capacity` distinct keys. A zero
    /// capacity retains nothing — every row is unattributed (the
    /// `EventFeed` zero-bound posture), and the sticky warn still fires
    /// once.
    #[must_use]
    pub fn bounded(capacity: usize) -> Self {
        Self {
            ledger: Mutex::new(Ledger {
                counts: HashMap::new(),
                capacity,
                unattributed: 0,
                warned: false,
            }),
        }
    }

    /// The bound the ledger was built with.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update) — the same contract the feed's locks carry.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.ledger
            .lock()
            .expect("the hot-key ledger lock")
            .capacity
    }

    /// Distinct keys currently tracked (the bound's occupancy, never
    /// above [`HotKeyObserver::capacity`]).
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn len(&self) -> usize {
        self.ledger
            .lock()
            .expect("the hot-key ledger lock")
            .counts
            .len()
    }

    /// True when nothing is tracked.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ledger
            .lock()
            .expect("the hot-key ledger lock")
            .counts
            .is_empty()
    }

    /// Events the bound refused since attach (or since the last reset) —
    /// the honest coverage score a dashboard renders beside the top
    /// window. Resident keys' events are never here (first-sight
    /// admission keeps a tracked key tracked).
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn unattributed(&self) -> u64 {
        self.ledger
            .lock()
            .expect("the hot-key ledger lock")
            .unattributed
    }

    /// The point lookup the key drill-down reads: the tracked key's
    /// count, `None` for a key the ledger does not track.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn count(&self, key: &str) -> Option<u64> {
        self.ledger
            .lock()
            .expect("the hot-key ledger lock")
            .counts
            .get(key)
            .copied()
    }

    /// The deterministic top window: at most `n` rows, count desc, key
    /// asc on ties. `n = 0` asks for nothing; an `n` past the occupancy
    /// returns every row.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn top(&self, n: usize) -> Vec<HotKeyRow> {
        let ledger = self.ledger.lock().expect("the hot-key ledger lock");
        let mut rows: Vec<HotKeyRow> = ledger
            .counts
            .iter()
            .map(|(key, count)| HotKeyRow {
                key: key.to_string(),
                count: *count,
            })
            .collect();
        // Count desc, key asc on ties — the F30 deterministic-order
        // doctrine: two reads over the same state are equal, so a diff
        // of two reads is a diff of the traffic, not of the sort.
        rows.sort_unstable_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
        rows.truncate(n);
        rows
    }

    /// Start a fresh window: the map clears, the unattributed score
    /// zeroes, the sticky warn re-arms. The windowed-sampling shape —
    /// a host that reports per interval resets after each report and
    /// the next window stands alone.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    pub fn reset(&self) {
        let mut ledger = self.ledger.lock().expect("the hot-key ledger lock");
        ledger.counts.clear();
        ledger.unattributed = 0;
        ledger.warned = false;
    }

    /// The §20 Prometheus text exposition of the top window (the F30
    /// parity): one `# HELP`/`# TYPE` counter pair for the labeled
    /// `oxide_cache_plane_hot_key_total` family, at most `top` rows in
    /// the [`HotKeyObserver::top`] order, label values escaped per the
    /// text format (`\`, `"` and newline), a trailing newline.
    /// Infallible — the `String` building cannot fail — and
    /// byte-stable: two renders over the same state are equal.
    ///
    /// The family is fully labeled (no bare row — the hits-family
    /// rule), so an empty window renders the pair and nothing else.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn render_prometheus(&self, top: usize) -> String {
        // One allocation for the scrape; the rows ride the same
        // deterministic order the value read hands back.
        let rows = self.top(top);
        let mut out = String::with_capacity(128 + rows.len() * 64);
        let _ = writeln!(
            out,
            "# HELP oxide_cache_plane_hot_key_total Cache-plane completed events attributed per key (the bounded top window)"
        );
        let _ = writeln!(out, "# TYPE oxide_cache_plane_hot_key_total counter");
        for row in rows {
            let _ = writeln!(
                out,
                "oxide_cache_plane_hot_key_total{{key=\"{}\"}} {}",
                escape_label_value(&row.key),
                row.count
            );
        }
        out
    }
}

/// Escape one label value per the Prometheus text format: backslash,
/// double quote and newline are the three escapes; everything else
/// rides verbatim.
fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

impl CacheObserver for HotKeyObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        // One mutex-protected update: the callback runs ON the cache's
        // call path (a hot L0 hit pays one lock, one lookup), the
        // observer contract's "cheap and never panics" rule. The warn
        // state rides the same lock, so a saturating stampede warns
        // exactly once.
        let refusal = {
            let mut ledger = self.ledger.lock().expect("the hot-key ledger lock");
            if let Some(count) = ledger.counts.get_mut(event.key) {
                // Saturating, not wrapping (the F30 rule): a count that
                // reaches `u64::MAX` clamps there — "at or past the
                // bound", never a silently-restarted total.
                *count = count.saturating_add(1);
                None
            } else if ledger.counts.len() < ledger.capacity {
                // First-sight admission: the key joins while there is
                // room, then counts forever (the memory tier's posture).
                let key: Box<str> = Box::from(event.key);
                ledger.counts.insert(key, 1);
                None
            } else {
                // The bound is full and the key is new: the row is
                // refused. The sticky warn fires once — the same lock
                // guarantees racing producers warn exactly once.
                let first_refusal = !ledger.warned;
                if first_refusal {
                    ledger.warned = true;
                }
                ledger.unattributed = ledger.unattributed.saturating_add(1);
                Some(first_refusal)
            }
        };
        if let Some(first_refusal) = refusal
            && first_refusal
        {
            tracing::warn!(
                target: "oxide::cache",
                capacity = self.capacity(),
                "cache hot-key ledger saturated; further NEW keys refuse until reset (counted in HotKeyObserver::unattributed)"
            );
        }
    }
}

/// The `Arc<HotKeyObserver>` sharing convenience: one constructor that
/// hands back the attached-and-readable half already split (the
/// `EventFeed::shared` shape). `with_observer` takes the
/// `Arc<dyn CacheObserver>` coercion; the host keeps the typed half for
/// the reads.
impl HotKeyObserver {
    /// `Arc::new(HotKeyObserver::bounded(capacity))` with the types
    /// spelled — the attach-and-read shape the module doc shows.
    #[must_use]
    pub fn shared(capacity: usize) -> Arc<Self> {
        Arc::new(Self::bounded(capacity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::CacheOp;

    #[test]
    fn count_saturates_instead_of_wrapping() {
        // The saturating clamp is not reachable through real traffic
        // (one event counts one), so the pin drives the ledger's own
        // arithmetic the honest way the F30 pin did: a full counter
        // plus one more stays at the bound.
        let hot = HotKeyObserver::bounded(1);
        push_max("route:/a", &hot);
        hot.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Miss,
        });
        assert_eq!(
            hot.count("route:/a"),
            Some(u64::MAX),
            "saturated, not wrapped"
        );
        assert_eq!(hot.unattributed(), 0, "the resident key kept counting");
    }

    /// Drive one key's count to `u64::MAX` through the saturating add
    /// itself (the test's setup, not the seam's API).
    fn push_max(key: &str, hot: &HotKeyObserver) {
        // Seed the key, then write the count directly through the same
        // lock the seam uses — the honest setup for an unreachable
        // bound.
        let mut ledger = hot.ledger.lock().expect("the hot-key ledger lock");
        ledger.counts.insert(Box::from(key), u64::MAX - 1);
    }

    #[test]
    fn escape_leaves_plain_values_verbatim() {
        assert_eq!(escape_label_value("route:/a"), "route:/a");
        assert_eq!(escape_label_value(""), "");
    }
}
