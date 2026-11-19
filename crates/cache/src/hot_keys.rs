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
//!
//! ## The admission gate (F39)
//!
//! The ledger tracks EVERY key it sees — right for a whole-plane
//! dashboard, wrong for a narrow one: a deploy-watch ledger
//! (`invalidate` + `coalesce` keys only) spends its capacity on the
//! hot-hit stream it would have counted and discarded, and a hot stream
//! fills the bound before the deploy keys it actually wants arrive. The
//! gate is the attribution-side narrowing (the F35 filter-surface shape
//! transposed — [`FeedGate`](crate::feed::FeedGate) narrowed the feed's
//! retention, this narrows the ledger's first-sight admission):
//! [`HotKeyGate`] sees the borrowed event BEFORE the ledger lock, so a
//! rejected row allocates nothing, never joins the map, never counts in
//! `unattributed()` and never fires the sticky warn — admission is the
//! ledger's configured shape, not a degradation. The bound applies
//! behind the gate unchanged: admitted keys refuse past capacity
//! exactly as before. The default is the gate spelled as a type
//! ([`AdmitAllKeys`]) — the ungated ledger row for row.
//!
//! ## The typed snapshot and the coverage exposition (F44)
//!
//! The ledger's reads were the four separate lock passes — `top(n)`,
//! `len()`, `capacity()`, `unattributed()` — a host composing them by
//! hand gets a torn answer when a producer runs between the calls: rows
//! from one instant, the coverage score from another. [`HotKeySnapshot`]
//! is the exporter's [`CacheMetricsSnapshot`](crate::CacheMetricsSnapshot)
//! posture transposed to the attribution arm: one `snapshot(window)`
//! read takes ONE lock pass and hands back the top window's rows beside
//! the whole-ledger aggregates (`tracked`, `capacity`, `unattributed`) —
//! Clone + Eq, so a diff of two snapshots is a diff of the traffic, not
//! of the read. [`HotKeyObserver::render_coverage_prometheus`] is the
//! exposition arm the module docs always named but never shipped: the
//! unattributed score renders as the counter a scrape diffs, the
//! occupancy and the bound render as gauges (current posture — the
//! [`crate::health::CacheHealth`] TYPE split), in the fixed family
//! order, byte-stable, infallible — the F21 posture holds. The
//! counter-family `render_prometheus` is untouched: the §20 scrape's
//! shape is byte-identical (the F40 default-shape doctrine), the
//! coverage arm composes beside it when a host wants it.
//!
//! ## The windowed hand-over (F45)
//!
//! [`HotKeyObserver::reset`]'s doc names the windowed-sampling shape: a
//! host that reports per interval resets after each report. The read
//! side of that shape was hand-rolled — `top(n)`, copy the rows,
//! `reset()` — three calls, three lock passes, and every event between
//! the first and the last is lost or double-counted (the torn hand-over
//! the F44 snapshot's one-pass read already fixed for the pure read).
//! [`HotKeyObserver::take`] composes the pair atomically: ONE lock pass
//! clones the whole map into the hand-over value (every tracked row —
//! the render bound is the caller's concern, the data is the ledger's)
//! and clears the ledger in the same critical section, so a window is
//! never both read and left. The fresh window stands alone exactly as
//! `reset` promises: the map empty, the unattributed score zeroed, the
//! sticky warn re-armed — the second window warns once more. A drained
//! key is gone: its next event admits fresh (first sight over), the
//! same posture a boot's first sight rides. The hand-over rides the
//! [`HotKeySnapshot`] value — the shape the F44 read answers, so a
//! host's report pipeline speaks one type for both the sample and the
//! take.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use crate::observer::{CacheEvent, CacheObserver};

/// The ledger's admission predicate (F39, the F35 filter-surface shape
/// transposed): the fanout and the exporters widen who hears an event,
/// the gate narrows which keys the ledger tracks.
///
/// The gate sees the BORROWED event — [`on_event`](CacheObserver::on_event)
/// consults it before the ledger lock, so a rejected row allocates
/// nothing and never joins the map. A rejection is admission, not
/// saturation: it never counts in [`HotKeyObserver::unattributed`] and
/// never fires the sticky warn (the warn is the bound's signal).
///
/// `Send + Sync + 'static` so the gate rides the `Arc`-shared ledger
/// across the cache's tasks. Implementations must be cheap and never
/// panic — the predicate runs ON the cache's call path, the same
/// contract the [`CacheObserver`](CacheObserver) callback carries.
pub trait HotKeyGate: Send + Sync + 'static {
    /// Whether the event's key joins the ledger. The `event` borrow
    /// lives only for the call.
    fn admits(&self, event: &CacheEvent<'_>) -> bool;
}

/// The default gate spelled as a type: every key joins the ledger. The
/// ungated ledger (`bounded`) is this predicate — `with_gate(Arc::new(
/// AdmitAllKeys))` matches it row for row (the F24 posture: the default
/// shape never moves; the type exists so a host can NAME the default).
#[derive(Debug, Default, Clone, Copy)]
pub struct AdmitAllKeys;

impl HotKeyGate for AdmitAllKeys {
    fn admits(&self, _event: &CacheEvent<'_>) -> bool {
        true
    }
}

/// The domain gate: attributes the row kinds named at construction and
/// refuses the rest, keyed on [`CacheEvent::kind`]'s stable names
/// (`"hit"`, `"miss"`, `"set"`, `"delete"`, `"invalidate"`,
/// `"coalesce"`). The deploy-watch ledger — `HotKeyKindGate::only(&[
/// "invalidate", "coalesce"])` tracks the keys a deploy touched, not
/// the hot-hit stream a narrow dashboard would have counted and
/// discarded.
#[derive(Debug, Clone)]
pub struct HotKeyKindGate {
    admitted: &'static [&'static str],
}

impl HotKeyKindGate {
    /// A gate attributing only the named kinds. The slice is held by
    /// reference (`&'static str` names are constants), so construction
    /// allocates nothing.
    #[must_use]
    pub fn only(admitted: &'static [&'static str]) -> Self {
        Self { admitted }
    }
}

impl HotKeyGate for HotKeyKindGate {
    fn admits(&self, event: &CacheEvent<'_>) -> bool {
        self.admitted.contains(&event.kind())
    }
}

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

/// The whole-ledger answer in one value (F44, the
/// [`CacheMetricsSnapshot`](crate::CacheMetricsSnapshot) posture
/// transposed to the attribution arm): the top window's rows in the
/// deterministic order beside the coverage aggregates. Taken under ONE
/// lock pass, so a racing producer cannot tear the rows from the
/// aggregates the way composing `top()` + `len()` + `capacity()` +
/// `unattributed()` by hand can.
///
/// Value-shaped by design (`Clone` + `Eq` + `Default`): a host copies
/// the snapshot before and after a window and diffs the values — a diff
/// of two snapshots is a diff of the traffic, not of the sort; a test
/// pins one; a dashboard ships one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotKeySnapshot {
    /// The top window's rows in the [`HotKeyObserver::top`] order (count
    /// desc, key asc on ties), at most the requested window's length.
    /// Window `0` carries no rows; a window past the occupancy carries
    /// every row.
    pub rows: Vec<HotKeyRow>,
    /// Distinct keys the ledger currently tracks (the bound's occupancy,
    /// never above `capacity`) — a whole-ledger fact, independent of the
    /// window that bounded `rows`.
    pub tracked: usize,
    /// The bound the ledger was built with ([`HotKeyObserver::capacity`]).
    pub capacity: usize,
    /// Events the bound refused since attach (or since the last reset) —
    /// the honest coverage score ([`HotKeyObserver::unattributed`]),
    /// riding beside the rows instead of a separate call.
    pub unattributed: u64,
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
    /// The F39 admission gate: `None` is the [`AdmitAllKeys`] default
    /// (the ungated shape, byte-identical), `Some` narrows the
    /// attribution before the ledger lock.
    gate: Option<Arc<dyn HotKeyGate>>,
}

impl std::fmt::Debug for HotKeyObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The ledger's occupancy and coverage score ARE the shape (the
        // `EventFeed` debug posture: name the shape, not the map's
        // internals); the gate is the attach shape.
        let ledger = self.ledger.lock().expect("the hot-key ledger lock");
        f.debug_struct("HotKeyObserver")
            .field("tracked", &ledger.counts.len())
            .field("capacity", &ledger.capacity)
            .field("unattributed", &ledger.unattributed)
            .field("gated", &self.gate.is_some())
            .finish()
    }
}

impl HotKeyObserver {
    /// A ledger tracking at most `capacity` distinct keys. A zero
    /// capacity retains nothing — every row is unattributed (the
    /// `EventFeed` zero-bound posture), and the sticky warn still fires
    /// once. The gate is the untyped default ([`AdmitAllKeys`]); narrow
    /// the attribution with [`HotKeyObserver::with_gate`].
    #[must_use]
    pub fn bounded(capacity: usize) -> Self {
        Self {
            ledger: Mutex::new(Ledger {
                counts: HashMap::new(),
                capacity,
                unattributed: 0,
                warned: false,
            }),
            gate: None,
        }
    }

    /// Narrow the ledger's attribution (F39): only events the gate
    /// admits join the map. The gate runs BEFORE the ledger lock, so a
    /// rejected row allocates nothing, never counts in
    /// [`HotKeyObserver::unattributed`] and never fires the sticky warn
    /// — admission is the ledger's configured shape, not a degradation.
    /// The bound applies behind the gate unchanged; the builder consumes
    /// and returns the ledger so the attach chain stays one expression.
    #[must_use = "the gate attaches through the builder chain; dropping it loses the narrowing"]
    pub fn with_gate(mut self, gate: Arc<dyn HotKeyGate>) -> Self {
        self.gate = Some(gate);
        self
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

    /// The F45 windowed hand-over: the whole window hands over as the
    /// same value shape [`snapshot`](HotKeyObserver::snapshot) answers,
    /// and the ledger starts a fresh one under the SAME lock pass —
    /// the read-and-reset pair the `reset` doc's windowed-sampling host
    /// hand-rolled, composed atomically. A window is never both read
    /// and left: events a racing producer completes during the call
    /// land in the FRESH window, never torn between the two.
    ///
    /// The hand-over carries every tracked row (at most `capacity` of
    /// them — the occupancy bound is the ledger's own), the occupancy
    /// and bound as they stood, and the unattributed score the window
    /// accumulated. Rows ride the deterministic order — count desc, key
    /// asc on ties — so the report pipeline renders without a sort.
    /// The render bound (`top(n)`'s window) stays the caller's concern:
    /// a report narrows the rows when it renders them, it does not
    /// silently drop them at the hand-over.
    ///
    /// The fresh window stands alone exactly as [`HotKeyObserver::reset`]
    /// promises: the map empty, the score zeroed, the sticky warn
    /// re-armed (a saturated ledger warns once more in the next window),
    /// and first-sight admission starts over — a drained key's next
    /// event inserts fresh.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use = "the window hands over as the value; dropping it loses the counts"]
    pub fn take(&self) -> HotKeySnapshot {
        let mut ledger = self.ledger.lock().expect("the hot-key ledger lock");
        let mut rows: Vec<HotKeyRow> = ledger
            .counts
            .iter()
            .map(|(key, count)| HotKeyRow {
                key: key.to_string(),
                count: *count,
            })
            .collect();
        // Count desc, key asc on ties — the deterministic order the
        // `top` and `snapshot` reads answer, so a host's report pipeline
        // speaks one shape end to end.
        rows.sort_unstable_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
        // The occupancy, bound and score are recorded BEFORE the clear —
        // the hand-over describes the window that closed, not the fresh
        // one opening.
        let taken = HotKeySnapshot {
            rows,
            tracked: ledger.counts.len(),
            capacity: ledger.capacity,
            unattributed: ledger.unattributed,
        };
        ledger.counts.clear();
        ledger.unattributed = 0;
        ledger.warned = false;
        taken
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

    /// The F44 typed read: the whole ledger answers as ONE value under
    /// ONE lock pass — the top window's rows beside the coverage
    /// aggregates — the posture [`snapshot`](HotKeyObserver::snapshot)'s
    /// doc holds. Hand-composing `top()` + `len()` + `capacity()` +
    /// `unattributed()` takes four lock passes a racing producer can
    /// tear between; this cannot be torn.
    ///
    /// `window` bounds only the rows: the aggregates are whole-ledger
    /// facts independent of it (the F41 doctrine — rows past the
    /// occupancy do not exist, so the honest cap keeps one read
    /// bounded). `snapshot(capacity())` is the full mirror; window `0`
    /// carries no rows and the aggregates still answer.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn snapshot(&self, window: usize) -> HotKeySnapshot {
        let ledger = self.ledger.lock().expect("the hot-key ledger lock");
        let mut rows: Vec<HotKeyRow> = ledger
            .counts
            .iter()
            .map(|(key, count)| HotKeyRow {
                key: key.to_string(),
                count: *count,
            })
            .collect();
        // Count desc, key asc on ties — the same deterministic order
        // `top` answers, so a snapshot's rows and a `top` read over the
        // same state agree row for row.
        rows.sort_unstable_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
        rows.truncate(window);
        HotKeySnapshot {
            rows,
            tracked: ledger.counts.len(),
            capacity: ledger.capacity,
            unattributed: ledger.unattributed,
        }
    }

    /// The F44 coverage exposition: the families the module docs always
    /// said a dashboard renders beside the top window, shipped as the
    /// §20 text-format arm a scrape composes beside
    /// [`render_prometheus`](HotKeyObserver::render_prometheus). The
    /// unattributed score is the **counter** a scrape diffs across
    /// windows; the occupancy and the bound are **gauges** (current
    /// posture, not deltas — the [`CacheHealth`](crate::health::CacheHealth)
    /// TYPE split). Fixed family order (score, occupancy, bound), one
    /// `# HELP`/`# TYPE` pair per family, a trailing newline — two
    /// renders over the same state are byte-equal. Infallible, so the
    /// scrape path carries no error shape (the F21 posture).
    ///
    /// The counter-family arm is untouched: the §20 scrape's existing
    /// shape stays byte-identical (the F40 default-shape doctrine); a
    /// host that wants the coverage families composes this beside it.
    ///
    /// # Panics
    ///
    /// Only if the ledger lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn render_coverage_prometheus(&self) -> String {
        // One lock pass for the three families; ~110 bytes of
        // HELP/TYPE/row per family — one allocation for the arm.
        let ledger = self.ledger.lock().expect("the hot-key ledger lock");
        let mut out = String::with_capacity(384);
        let _ = writeln!(
            out,
            "# HELP oxide_cache_plane_hot_key_unattributed_total Cache-plane completed events the hot-key ledger's bound refused (the coverage score)"
        );
        let _ = writeln!(
            out,
            "# TYPE oxide_cache_plane_hot_key_unattributed_total counter"
        );
        let _ = writeln!(
            out,
            "oxide_cache_plane_hot_key_unattributed_total {}",
            ledger.unattributed
        );
        let _ = writeln!(
            out,
            "# HELP oxide_cache_plane_hot_key_keys Hot-key keys the ledger currently tracks (the bound's occupancy)"
        );
        let _ = writeln!(out, "# TYPE oxide_cache_plane_hot_key_keys gauge");
        let _ = writeln!(
            out,
            "oxide_cache_plane_hot_key_keys {}",
            ledger.counts.len()
        );
        let _ = writeln!(
            out,
            "# HELP oxide_cache_plane_hot_key_capacity The hot-key ledger's configured bound"
        );
        let _ = writeln!(out, "# TYPE oxide_cache_plane_hot_key_capacity gauge");
        let _ = writeln!(
            out,
            "oxide_cache_plane_hot_key_capacity {}",
            ledger.capacity
        );
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
        // The F39 gate first, on the BORROWED event: a rejected row
        // allocates nothing, never joins the map, never counts in
        // `unattributed()` and never fires the warn — admission is the
        // ledger's configured shape, not a degradation. `None` (the
        // default) is the `AdmitAllKeys` shape — one `Option` branch,
        // the ungated path byte-identical.
        if let Some(gate) = &self.gate
            && !gate.admits(event)
        {
            return;
        }
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
/// the reads. A gated ledger rides the same constructor —
/// `HotKeyObserver::gated(capacity, gate)` is
/// `Arc::new(HotKeyObserver::bounded(capacity).with_gate(gate))` with
/// the shapes spelled.
impl HotKeyObserver {
    /// `Arc::new(HotKeyObserver::bounded(capacity))` with the types
    /// spelled — the attach-and-read shape the module doc shows.
    #[must_use]
    pub fn shared(capacity: usize) -> Arc<Self> {
        Arc::new(Self::bounded(capacity))
    }

    /// The gated ledger's shared shape: the F39 narrowing over the
    /// attach-and-read constructor, one expression.
    #[must_use]
    pub fn gated(capacity: usize, gate: Arc<dyn HotKeyGate>) -> Arc<Self> {
        Arc::new(Self::bounded(capacity).with_gate(gate))
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
