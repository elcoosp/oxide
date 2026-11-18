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

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    CacheEvent, CacheObserver, CacheOp, Entry, EventFeed, FanoutCacheObserver, HotKeyObserver,
    MetricsCacheObserver, Policy, TieredCache,
};

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
