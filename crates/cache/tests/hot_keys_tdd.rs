//! TDD suite for the cache plane's hot-key attribution (`hot_keys.rs`).
//! Written first per the house TDD rule: every pin failed on unresolved
//! symbols (E0432 over `HotKeyObserver` / `HotKeyRow`) before the
//! attribution seam existed.
//!
//! The contract the pins hold:
//!
//! 1. [`HotKeyObserver`](oxide_fw_cache::HotKeyObserver) is the F28
//!    seam's attribution arm: a `CacheObserver` impl that answers the
//!    per-key question the aggregate counters cannot ("which keys
//!    dominate the plane's traffic") — the dashboard-batcher shape the
//!    `observer.rs` module doc names as a custom impl, shipped once.
//! 2. Every completed event attributes to its key, whatever the op:
//!    hits, misses, sets, deletes, invalidations and coalesce rows all
//!    count toward the key they ran against.
//! 3. The map is bounded with the feed's honest-saturation posture: a
//!    resident key keeps counting past the bound, a NEW key admits only
//!    while there is room (first-sight admission, the memory tier's
//!    TinyLFU-ish posture), and past the bound the arriving rows are
//!    unattributed — counted, never silent ([`HotKeyObserver::unattributed`]
//!    keeps the score; the first refusal warns once under `oxide::cache`,
//!    the F31 sticky rule).
//! 4. [`HotKeyObserver::top`] is the deterministic read: count desc,
//!    key asc on ties, at most `n` rows — two reads over the same state
//!    are equal, so a snapshot diff is a diff of the traffic, not the
//!    sort.
//! 5. `render_prometheus` is the exposition arm (the F30 parity): the
//!    labeled `oxide_cache_plane_hot_key_total` counter family over the
//!    top window, label values escaped per the text format, a fixed
//!    row shape, a trailing newline — byte-stable re-renders.
//! 6. [`HotKeyObserver::reset`] starts a fresh window: counts cleared,
//!    the unattributed score zeroed, the sticky warn re-armed.
//! 7. A zero capacity retains nothing and refuses everything (the
//!    `EventFeed` zero-bound posture).
//! 8. Over real cache traffic through the F28 builder the seam agrees
//!    with the exporter: the fan of both counts the same events.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{
    CacheEvent, CacheObserver, CacheOp, Entry, EventFeed, FanoutCacheObserver, HotKeyObserver,
    HotKeyRow, MetricsCacheObserver, Policy, TieredCache,
};
use tracing_subscriber::prelude::*;

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the wall clock runs")
            .as_millis()
            .try_into()
            .unwrap_or_default(),
        etag: "\"v1\"".into(),
    }
}

/// Drive one event straight into the observer (the exporter tests' shape
/// — the pin names the payload, not the tier mechanics).
fn push(obs: &HotKeyObserver, key: &str, op: CacheOp) {
    obs.on_event(&CacheEvent { key, op });
}

// --------------------------------------------------------------- the seam pin

/// The attribution arm IS a `CacheObserver`, attachable through the F28
/// builder, and every completed event attributes to its key over real
/// traffic — set, hit and miss alike.
#[tokio::test]
async fn hot_keys_is_a_cache_observer_and_attributes_real_traffic() {
    let hot = Arc::new(HotKeyObserver::bounded(64));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(hot.clone()));

    cache
        .set("route:/a", entry(b"hello", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/missing").await.expect("the memory tier");

    let rows = hot.top(10);
    assert_eq!(rows.len(), 2, "two distinct keys ran");
    assert_eq!(rows[0].key, "route:/a", "the hit key dominates");
    assert_eq!(rows[0].count, 2, "the set AND the hit attribute to the key");
    assert_eq!(rows[1].key, "route:/missing");
    assert_eq!(rows[1].count, 1, "the miss attributes too");
    assert_eq!(hot.unattributed(), 0, "the bound never engaged");
    assert!(cache.health().observer_attached);
}

/// The op payload does not matter to the ledger — hits, misses, sets,
/// deletes, invalidations and coalesce rows all attribute to the key
/// they ran against (the pin names the rule so an exporter-style
/// per-op split cannot sneak in).
#[test]
fn every_op_kind_attributes_to_its_key() {
    let hot = HotKeyObserver::bounded(64);
    push(&hot, "route:/a", CacheOp::Set { bytes: 8 });
    push(&hot, "route:/a", CacheOp::Miss);
    push(
        &hot,
        "route:/a",
        CacheOp::Hit {
            layer: oxide_fw_cache::Layer::L0,
            stale: false,
            bytes: 8,
        },
    );
    push(&hot, "route:/a", CacheOp::Delete);
    push(&hot, "route:/a", CacheOp::Delete);
    push(
        &hot,
        "route:/b",
        CacheOp::Invalidate {
            tag: "content:posts".into(),
            soft: true,
            count: 3,
        },
    );
    push(&hot, "route:/b", CacheOp::Coalesce { followers: 2 });

    let rows = hot.top(10);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        oxide_fw_cache::HotKeyRow {
            key: "route:/a".into(),
            count: 5,
        }
    );
    assert_eq!(
        rows[1].count, 2,
        "the aggregate rows attribute once per row"
    );
}

// --------------------------------------------------------------- the bound pins

/// Past the bound a RESIDENT key keeps counting (first-sight admission —
/// the memory tier's posture: the ledger never loses a key it already
/// tracks) while a NEW key is refused.
#[test]
fn resident_keys_keep_counting_past_the_bound() {
    let hot = HotKeyObserver::bounded(2);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    // The bound is full. Residents still count; the third key is refused.
    push(&hot, "route:/a", CacheOp::Miss);
    assert_eq!(
        hot.count("route:/a"),
        Some(2),
        "the resident key kept counting"
    );
    assert_eq!(hot.len(), 2, "the bound holds");
    push(&hot, "route:/c", CacheOp::Miss);
    assert_eq!(hot.count("route:/c"), None, "the new key never admitted");
    assert_eq!(hot.unattributed(), 1, "the refusal keeps the score");
}

/// Each refused event adds one to the unattributed score — the honest
/// read a dashboard renders beside the top window ("the ledger covers
/// this much of the traffic").
#[test]
fn unattributed_counts_every_refused_event() {
    let hot = HotKeyObserver::bounded(1);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);
    assert_eq!(hot.len(), 1);
    assert_eq!(hot.unattributed(), 3, "three refused events, three counts");
}

/// A zero capacity retains nothing — every event is unattributed (the
/// `EventFeed` zero-bound posture), the observer still attaches and
/// never panics.
#[test]
fn zero_capacity_retains_nothing() {
    let hot = HotKeyObserver::bounded(0);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    assert!(hot.is_empty());
    assert_eq!(hot.len(), 0);
    assert_eq!(hot.count("route:/a"), None);
    assert_eq!(hot.unattributed(), 2);
    assert_eq!(hot.capacity(), 0);
}

// --------------------------------------------------------------- the read pins

/// `top` is the deterministic read: count desc, key asc on ties, at
/// most `n` rows. Two reads over the same state are equal.
#[test]
fn top_sorts_count_desc_then_key_asc_and_bounds_n() {
    let hot = HotKeyObserver::bounded(8);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);

    let rows = hot.top(10);
    assert_eq!(
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec!["route:/c", "route:/a", "route:/b"],
        "count desc; the a/b tie would break key asc"
    );
    // The a/b tiebreak: same count, key asc.
    let tied = HotKeyObserver::bounded(8);
    push(&tied, "route:/b", CacheOp::Miss);
    push(&tied, "route:/a", CacheOp::Miss);
    let rows = tied.top(10);
    assert_eq!(
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
        vec!["route:/a", "route:/b"],
        "equal counts break by key asc"
    );
    assert_eq!(tied.top(10), tied.top(10), "re-reads are byte-equal");

    let hot = HotKeyObserver::bounded(8);
    for key in ["a", "b", "c", "d"] {
        push(&hot, key, CacheOp::Miss);
    }
    assert_eq!(hot.top(2).len(), 2, "n bounds the rows");
    assert_eq!(hot.top(0).len(), 0, "zero asks for nothing");
    assert_eq!(hot.top(usize::MAX).len(), 4, "a huge n is the whole window");
}

/// `count` is the point lookup the dashboard's key drill-down reads:
/// tracked keys answer, untracked keys say `None`.
#[test]
fn count_answers_tracked_keys_only() {
    let hot = HotKeyObserver::bounded(4);
    push(&hot, "route:/a", CacheOp::Set { bytes: 1 });
    assert_eq!(hot.count("route:/a"), Some(1));
    assert_eq!(hot.count("route:/missing"), None);
    assert!(!hot.is_empty(), "the tracked key holds the ledger");
}

// --------------------------------------------------------------- the reset pin

/// `reset` starts a fresh window: counts cleared, the unattributed score
/// zeroed — and the sticky warn re-armed (a fresh window may warn again;
/// the counts pin holds the state, the warn rides the same reset).
#[test]
fn reset_starts_a_fresh_window() {
    let hot = HotKeyObserver::bounded(1);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    assert_eq!(hot.unattributed(), 1);

    hot.reset();
    assert!(hot.is_empty());
    assert_eq!(hot.count("route:/a"), None);
    assert_eq!(hot.unattributed(), 0);

    // The fresh window admits afresh — the bound re-engages from zero.
    push(&hot, "route:/c", CacheOp::Miss);
    assert_eq!(hot.count("route:/c"), Some(1), "the fresh window admits");
}

// --------------------------------------------------------------- the debug pin

/// The Debug names the ledger shape (the `EventFeed` debug posture):
/// tracked/capacity/unattributed, not the map's internals.
#[test]
fn debug_names_the_ledger_shape() {
    let hot = HotKeyObserver::bounded(4);
    push(&hot, "route:/a", CacheOp::Miss);
    let rendered = format!("{hot:?}");
    assert!(rendered.starts_with("HotKeyObserver"), "{rendered}");
    assert!(rendered.contains("tracked: 1"), "{rendered}");
    assert!(rendered.contains("capacity: 4"), "{rendered}");
    assert!(rendered.contains("unattributed: 0"), "{rendered}");
}

// --------------------------------------------------------------- the convenience pin

/// `shared` is the attach-and-read convenience: the `Arc` the tiers'
/// field stores, ready for `with_observer` (the `EventFeed::shared`
/// shape).
#[tokio::test]
async fn shared_returns_the_attached_half() {
    let hot = HotKeyObserver::shared(8);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(hot.clone()));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    assert_eq!(hot.count("route:/a"), Some(1));
}

// --------------------------------------------------------------- the composite pin

/// Over real traffic the composed consumers agree (the fanout composite
/// story): the exporter's counters and the hot-key ledger describe the
/// SAME events — every attributed event is one counted event.
#[tokio::test]
async fn fanout_composite_exporter_and_ledger_agree() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let hot = Arc::new(HotKeyObserver::bounded(64));
    let fan = Arc::new(FanoutCacheObserver::new(vec![
        metrics.clone() as Arc<dyn CacheObserver>,
        hot.clone() as Arc<dyn CacheObserver>,
    ]));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/missing").await.expect("the memory tier");

    let snap = metrics.snapshot();
    let exporter_total = snap.sets + snap.hits_l0 + snap.misses;
    let attributed: u64 = hot.top(usize::MAX).iter().map(|r| r.count).sum();
    assert_eq!(
        attributed, exporter_total,
        "the ledger and the exporter describe the same events"
    );
}

// --------------------------------------------------------------- the feed-composition pin

/// The attribution arm composes beside the feed behind one fan (the
/// dashboard boot's attach shape): the feed drains its FIFO, the ledger
/// keeps its counts — neither sees the other's cost.
#[tokio::test]
async fn hot_keys_composes_beside_the_feed() {
    let feed = Arc::new(EventFeed::bounded(64));
    let hot = Arc::new(HotKeyObserver::bounded(64));
    let fan = FanoutCacheObserver::shared(vec![
        feed.clone() as Arc<dyn CacheObserver>,
        hot.clone() as Arc<dyn CacheObserver>,
    ]);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));
    for key in ["route:/a", "route:/b", "route:/c"] {
        cache
            .set(key, entry(b"x", &[]))
            .await
            .expect("the memory tier accepts the entry");
    }
    assert_eq!(feed.len(), 3, "the feed queued the rows");
    assert_eq!(hot.len(), 3, "the ledger tracked the keys");
    assert_eq!(
        hot.top(1)[0].count,
        1,
        "uniform traffic keeps the counts flat"
    );
}

// --------------------------------------------------------------- the exposition pins

/// `render_prometheus` is the F30-parity exposition arm: the labeled
/// `oxide_cache_plane_hot_key_total` counter family over the top window,
/// a `# HELP`/`# TYPE` pair, a trailing newline, byte-stable re-renders.
#[test]
fn exposition_renders_the_labeled_family_deterministically() {
    let hot = HotKeyObserver::bounded(8);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);

    let out = hot.render_prometheus(10);
    assert!(
        out.starts_with("# HELP oxide_cache_plane_hot_key_total "),
        "{out}"
    );
    assert!(
        out.contains("# TYPE oxide_cache_plane_hot_key_total counter\n"),
        "{out}"
    );
    // The rows ride the top order: count desc, key asc on ties.
    assert!(
        out.contains("oxide_cache_plane_hot_key_total{key=\"route:/a\"} 2\n"),
        "{out}"
    );
    assert!(
        out.contains("oxide_cache_plane_hot_key_total{key=\"route:/b\"} 1\n"),
        "{out}"
    );
    // One row per top entry, no bare row (the hits-family rule: the
    // family is fully labeled).
    let rows = out
        .lines()
        .filter(|line| line.starts_with("oxide_cache_plane_hot_key_total{"))
        .count();
    assert_eq!(rows, 2);
    assert!(out.ends_with('\n'), "the trailing newline");
    assert_eq!(out, hot.render_prometheus(10), "re-renders are byte-equal");
}

/// The top bound applies to the exposition too, and label values are
/// escaped per the text format (`\`, `"`, newline) — a key with a quote
/// renders as data, not as a broken scrape.
#[test]
fn exposition_escapes_label_values_and_bounds_the_window() {
    let hot = HotKeyObserver::bounded(8);
    push(&hot, "route:/\"quoted\"", CacheOp::Miss);
    push(&hot, "route:/back\\slash", CacheOp::Miss);

    let out = hot.render_prometheus(10);
    assert!(
        out.contains("oxide_cache_plane_hot_key_total{key=\"route:/\\\"quoted\\\"\"} 1\n"),
        "{out}"
    );
    assert!(
        out.contains("oxide_cache_plane_hot_key_total{key=\"route:/back\\\\slash\"} 1\n"),
        "{out}"
    );

    // The n bound: only the top window renders.
    let hot = HotKeyObserver::bounded(8);
    for key in ["a", "b", "c"] {
        push(&hot, key, CacheOp::Miss);
    }
    let out = hot.render_prometheus(2);
    let rows = out
        .lines()
        .filter(|line| line.starts_with("oxide_cache_plane_hot_key_total{"))
        .count();
    assert_eq!(rows, 2, "the exposition rides the top(n) window");
}

// --------------------------------------------------------------- the snapshot pins (F44)

/// The F44 typed read: the whole ledger answers as ONE value under ONE
/// lock pass — the top window's rows in the deterministic order beside
/// the coverage aggregates (`tracked`, `capacity`, `unattributed`) — the
/// exporter's `CacheMetricsSnapshot` posture transposed to the
/// attribution arm. Composing `top()` + `len()` + `capacity()` +
/// `unattributed()` by hand takes four lock passes a racing producer can
/// tear between; the snapshot cannot be torn. Two reads over the same
/// state are equal, so a diff of two snapshots is a diff of the traffic,
/// not of the read.
#[test]
fn snapshot_answers_the_whole_ledger_in_one_value() {
    let hot = HotKeyObserver::bounded(8);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Set { bytes: 8 });

    let snap = hot.snapshot(10);
    assert_eq!(snap.rows.len(), 2, "the top window rides the value");
    assert_eq!(
        snap.rows[0],
        HotKeyRow {
            key: "route:/a".to_string(),
            count: 2,
        },
        "count desc"
    );
    assert_eq!(
        snap.rows[1],
        HotKeyRow {
            key: "route:/b".to_string(),
            count: 1,
        },
        "key asc on ties"
    );
    assert_eq!(snap.tracked, 2, "distinct keys the ledger holds");
    assert_eq!(snap.capacity, 8, "the bound the ledger was built with");
    assert_eq!(snap.unattributed, 0);
    assert_eq!(
        snap,
        hot.snapshot(10),
        "re-reads over the same state are equal"
    );
}

/// The window bounds the rows the value carries — and nothing else: the
/// coverage aggregates are whole-ledger facts independent of the window
/// (the F41 doctrine: rows past the occupancy do not exist, the honest
/// cap keeps one read bounded; window `0` asks for no rows but the
/// aggregates still answer).
#[test]
fn snapshot_window_bounds_the_rows_not_the_aggregates() {
    let hot = HotKeyObserver::bounded(8);
    for key in ["a", "b", "c"] {
        push(&hot, key, CacheOp::Miss);
    }
    let narrow = hot.snapshot(1);
    assert_eq!(narrow.rows.len(), 1, "the window bounds the rows");
    assert_eq!(narrow.rows[0].key, "a", "count desc, key asc on ties");
    assert_eq!(narrow.tracked, 3, "the aggregates are whole-ledger facts");
    let wide = hot.snapshot(0);
    assert!(wide.rows.is_empty(), "window 0 asks for no rows");
    assert_eq!(wide.tracked, 3);
    assert_eq!(wide.unattributed, 0);
}

/// The coverage score rides the value: refusals land in `unattributed`
/// exactly as the direct read answers — beside the rows, in one lock
/// pass (the F31 honest score, now part of the snapshot).
#[test]
fn snapshot_carries_the_unattributed_score() {
    let hot = HotKeyObserver::bounded(1);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);
    let snap = hot.snapshot(10);
    assert_eq!(snap.unattributed, 2, "two refused rows");
    assert_eq!(snap.tracked, 1);
    assert_eq!(snap.rows.len(), 1);
}

/// The zero-bound ledger (the `EventFeed` zero-bound posture): the value
/// carries the empty shape honestly — no rows, everything unattributed,
/// the bound rendered as built.
#[test]
fn snapshot_on_the_zero_bound_ledger() {
    let hot = HotKeyObserver::bounded(0);
    push(&hot, "route:/a", CacheOp::Miss);
    let snap = hot.snapshot(10);
    assert_eq!(snap.rows, [] as [oxide_fw_cache::HotKeyRow; 0]);
    assert_eq!(snap.unattributed, 1);
    assert_eq!(snap.tracked, 0);
    assert_eq!(snap.capacity, 0);
}

// --------------------------------------------------------------- the coverage exposition pins (F44)

/// `render_coverage_prometheus` is the coverage arm (the F32 gauge
/// posture): the unattributed score renders as the counter a scrape
/// diffs, the occupancy and the bound render as gauges (current
/// posture, not deltas). These are the families the module docs have
/// always said a dashboard renders beside the top window — shipped so
/// an operator's scrape carries them instead of a hand-rolled reader
/// over four separate calls. Fixed family order (`# HELP`/`# TYPE`
/// pairs), a trailing newline, byte-stable re-renders.
#[test]
fn coverage_exposition_renders_the_families_deterministically() {
    let hot = HotKeyObserver::bounded(1);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    let out = hot.render_coverage_prometheus();
    assert!(
        out.starts_with("# HELP oxide_cache_plane_hot_key_unattributed_total "),
        "{out}"
    );
    assert!(
        out.contains("# TYPE oxide_cache_plane_hot_key_unattributed_total counter\n"),
        "{out}"
    );
    assert!(
        out.contains("oxide_cache_plane_hot_key_unattributed_total 1\n"),
        "{out}"
    );
    assert!(
        out.contains("# HELP oxide_cache_plane_hot_key_keys "),
        "{out}"
    );
    assert!(
        out.contains("# TYPE oxide_cache_plane_hot_key_keys gauge\n"),
        "{out}"
    );
    assert!(out.contains("oxide_cache_plane_hot_key_keys 1\n"), "{out}");
    assert!(
        out.contains("# TYPE oxide_cache_plane_hot_key_capacity gauge\n"),
        "{out}"
    );
    assert!(
        out.contains("oxide_cache_plane_hot_key_capacity 1\n"),
        "{out}"
    );
    // The fixed family order: score, occupancy, bound.
    let score = out
        .find("oxide_cache_plane_hot_key_unattributed_total 1")
        .expect("the score family renders");
    let keys = out
        .find("oxide_cache_plane_hot_key_keys 1")
        .expect("the occupancy family renders");
    let bound = out
        .find("oxide_cache_plane_hot_key_capacity 1")
        .expect("the bound family renders");
    assert!(score < keys && keys < bound, "{out}");
    assert!(out.ends_with('\n'), "the trailing newline");
    assert_eq!(
        out,
        hot.render_coverage_prometheus(),
        "re-renders are byte-equal"
    );
}

/// The arm tracks the live state: refusals land in the counter family,
/// the occupancy gauge follows the map, and a reset zeroes both (the
/// posture the gauge TYPE names — current state, not deltas).
#[test]
fn coverage_exposition_tracks_the_live_state() {
    let hot = HotKeyObserver::bounded(2);
    push(&hot, "a", CacheOp::Miss);
    push(&hot, "b", CacheOp::Miss);
    push(&hot, "c", CacheOp::Miss);
    let out = hot.render_coverage_prometheus();
    assert!(
        out.contains("oxide_cache_plane_hot_key_unattributed_total 1\n"),
        "{out}"
    );
    assert!(out.contains("oxide_cache_plane_hot_key_keys 2\n"), "{out}");
    hot.reset();
    let out = hot.render_coverage_prometheus();
    assert!(
        out.contains("oxide_cache_plane_hot_key_unattributed_total 0\n"),
        "{out}"
    );
    assert!(out.contains("oxide_cache_plane_hot_key_keys 0\n"), "{out}");
    assert!(
        out.contains("oxide_cache_plane_hot_key_capacity 2\n"),
        "{out}"
    );
}

// --------------------------------------------------------------- the take pins (F45)

/// The console sink the warn pins read: a `MakeWriter` over a shared
/// byte buffer (the `hot_key_gate_tdd` shape — the helpers ride here
/// because test targets do not share code).
#[derive(Clone)]
struct ConsoleWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for ConsoleWriter {
    type Writer = ConsoleSink;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleSink(self.0.clone())
    }
}

struct ConsoleSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for ConsoleSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("console buffer")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Count the rendered warn rows the bound's sticky rule leaves.
fn warn_rows(buffer: &ConsoleWriter) -> usize {
    let bytes = buffer.0.lock().expect("console buffer");
    std::str::from_utf8(&bytes)
        .expect("the fmt render is utf-8")
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("saturated"))
        .count()
}

/// The capture pins' registration lock: two subscriber installs racing
/// rebuild the tracing callsite cache, and a warn fired mid-rebuild is
/// dropped — the sticky-warn pins register one at a time so each drive
/// runs against a warm cache (the flake the parallel suite hit).
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

/// The F45 windowed hand-over: `take()` returns the whole window as the
/// same value shape the read answers (every tracked row in the
/// deterministic order — the hand-over is the DATA, the render bound is
/// the caller's concern) and the ledger starts a fresh one under the
/// SAME lock pass: the map clears, the unattributed score zeroes, and
/// the counts are never both read and left (the torn hand-over
/// hand-composing `top()` + `reset()` suffers: events between the two
/// calls are lost or double-counted).
#[test]
fn take_hands_over_the_whole_window_and_starts_a_fresh_one() {
    let hot = HotKeyObserver::bounded(2);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/b", CacheOp::Miss);
    push(&hot, "route:/c", CacheOp::Miss);

    let taken = hot.take();
    assert_eq!(
        taken.rows,
        vec![
            HotKeyRow {
                key: "route:/a".to_string(),
                count: 2,
            },
            HotKeyRow {
                key: "route:/b".to_string(),
                count: 1,
            },
        ],
        "every tracked row rides the hand-over, top order"
    );
    assert_eq!(taken.tracked, 2, "the occupancy the window drained");
    assert_eq!(taken.capacity, 2, "the bound the window ran under");
    assert_eq!(taken.unattributed, 1, "the score the window accumulated");

    // The fresh window stands alone: nothing tracked, the score zeroed,
    // the point lookup answers honestly.
    assert!(hot.is_empty(), "the map cleared");
    assert_eq!(hot.unattributed(), 0, "the score zeroed");
    assert_eq!(hot.count("route:/a"), None);
}

/// The hand-over restarts FIRST-SIGHT admission: a resident key the
/// window drained is gone — its next event inserts fresh (count 1), the
/// same posture a boot's first sight rides. (`reset`'s documented
/// windowed-sampling shape, now with the read-and-reset composed.)
#[test]
fn take_restarts_first_sight_admission() {
    let hot = HotKeyObserver::bounded(8);
    push(&hot, "route:/a", CacheOp::Miss);
    push(&hot, "route:/a", CacheOp::Miss);
    let _ = hot.take();
    push(&hot, "route:/a", CacheOp::Miss);
    assert_eq!(
        hot.count("route:/a"),
        Some(1),
        "the drained key admits fresh, not resident"
    );
}

/// The sticky warn re-arms with the fresh window (the `reset` rule):
/// saturate → one warn; take; saturate again → the second window warns
/// once more. A host that reports per interval keeps its honest signal.
#[test]
fn take_rearms_the_sticky_warn() {
    let _capture = CAPTURE_LOCK.lock().expect("the capture lock");
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let hot = HotKeyObserver::bounded(1);

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        push(&hot, "route:/a", CacheOp::Miss);
        push(&hot, "route:/b", CacheOp::Miss);
        assert_eq!(warn_rows(&buffer), 1, "the first window warns once");
        let _ = hot.take();
        // The fresh window has the bound's room back (the map cleared),
        // so the re-arm drive refills it first: the second window's
        // FIRST refusal is the re-armed warn.
        push(&hot, "route:/c", CacheOp::Miss);
        push(&hot, "route:/d", CacheOp::Miss);
        assert_eq!(warn_rows(&buffer), 2, "the fresh window re-arms the warn");
    });
}

/// The empty hand-over: an empty ledger hands an empty window (the
/// zero-bound posture's honest shape — no fabricated rows, nothing to
/// zero) and the ledger stays empty.
#[test]
fn take_on_an_empty_ledger_hands_an_empty_window() {
    let hot = HotKeyObserver::bounded(8);
    let taken = hot.take();
    assert_eq!(taken.rows, [] as [oxide_fw_cache::HotKeyRow; 0]);
    assert_eq!(taken.tracked, 0);
    assert_eq!(taken.capacity, 8);
    assert_eq!(taken.unattributed, 0);
    assert!(hot.is_empty());
}
