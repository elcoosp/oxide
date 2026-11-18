//! TDD suite for the cache plane's coalesce parity row (F33): the §11.3
//! single-flight joins join the §20 push channel. Written first per the
//! house TDD rule: every pin failed on unresolved symbols (E0599 over
//! `CacheOp::Coalesce` and the new `MetricsCacheObserver` fields) before
//! the row existed.
//!
//! The contract the pins hold:
//!
//! 1. The pull channel's `coalesced` counter has a push twin:
//!    [`TieredCache::record_coalesced`](oxide_fw_cache::TieredCache::record_coalesced)
//!    — the entry point the host's flight owner calls when a flight
//!    completes — now emits one [`CacheOp::Coalesce`](oxide_fw_cache::CacheOp)
//!    row through the attached observer, beside the counter bump. The push
//!    channel sees what the pull channel sees.
//! 2. The row is one aggregate per completed flight (the
//!    `CacheOp::Invalidate` posture — one row, not one per follower), the
//!    key names the flight's key, and `followers` is the count the
//!    owner's publish reached. A zero-follower flight still completes and
//!    still records — the join count is honest data, not a gate.
//! 3. The in-crate consumers inherit the row for free:
//!    `CacheEvent::kind()` names it `"coalesce"`, `TracingCacheObserver`
//!    emits it under the `oxide::cache` target with the `followers`
//!    field, `MetricsCacheObserver` counts the flights and the joins into
//!    the `oxide_cache_plane_coalesce_events_total` /
//!    `oxide_cache_plane_coalesced_followers_total` families (monotone,
//!    saturating — the F30 byte-accounting posture), and `EventFeed`
//!    carries it as any other row.
//! 4. `stats().coalesced` keeps its meaning — the counter the pull
//!    channel always served — so the two channels never disagree.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxide_fw_cache::{
    CacheEvent, CacheMetricsSnapshot, CacheObserver, CacheOp, Entry, EventFeed,
    MetricsCacheObserver, Policy, TieredCache,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn entry() -> Entry {
    Entry {
        body: b"coalesced".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

/// The seam-tests' counting observer (the `metrics_tdd` posture): capture
/// the rows, compare values.
#[derive(Default)]
struct CountingObserver {
    captured: Mutex<Vec<(String, CacheOp)>>,
}

impl CacheObserver for CountingObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.captured
            .lock()
            .expect("the test is the only contender")
            .push((event.key.to_string(), event.op.clone()));
    }
}

#[tokio::test]
async fn the_coalesce_row_rides_the_push_channel() {
    let obs = Arc::new(CountingObserver::default());
    let cache = TieredCache::new(1 << 20).with_observer(obs.clone());
    cache.set("route:/a", entry()).await.unwrap();

    // The host's flight owner completes: three followers joined the
    // owner's render for this key.
    cache.record_coalesced("route:/a", 3);

    let rows = obs
        .captured
        .lock()
        .expect("the test is the only contender")
        .clone();
    let coalesce_rows: Vec<_> = rows
        .iter()
        .filter(|(_, op)| matches!(op, CacheOp::Coalesce { .. }))
        .collect();
    assert_eq!(coalesce_rows.len(), 1, "one aggregate row per flight");
    assert_eq!(coalesce_rows[0].0, "route:/a");
    assert_eq!(
        coalesce_rows[0].1,
        CacheOp::Coalesce { followers: 3 },
        "the row carries the join count"
    );
    // The pull channel keeps its meaning, and the two agree.
    assert_eq!(cache.stats().coalesced, 3);
}

#[tokio::test]
async fn the_kind_name_is_stable() {
    let k = CacheEvent {
        key: "k",
        op: CacheOp::Coalesce { followers: 1 },
    }
    .kind();
    assert_eq!(k, "coalesce");
}

#[tokio::test]
async fn zero_followers_still_records_the_completed_flight() {
    let obs = Arc::new(CountingObserver::default());
    let cache = TieredCache::new(1 << 20).with_observer(obs.clone());
    cache.record_coalesced("route:/lonely", 0);

    let rows = obs
        .captured
        .lock()
        .expect("the test is the only contender")
        .clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, CacheOp::Coalesce { followers: 0 });
    assert_eq!(cache.stats().coalesced, 0);
}

#[test]
fn tracing_observer_takes_the_row() {
    let obs = oxide_fw_cache::TracingCacheObserver::new();
    // No subscriber installed: the rows are no-ops (the tracing
    // contract) — the pin is "takes the row without panic".
    obs.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Coalesce { followers: 2 },
    });
}

#[tokio::test]
async fn the_metrics_exporter_counts_the_flights_and_the_joins() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = TieredCache::new(1 << 20).with_observer(metrics.clone());
    cache.set("route:/a", entry()).await.unwrap();
    cache.record_coalesced("route:/a", 3);
    cache.record_coalesced("route:/b", 2);

    let snap: CacheMetricsSnapshot = metrics.snapshot();
    assert_eq!(snap.coalesce_events, 2, "one counter per completed flight");
    assert_eq!(snap.coalesced_followers, 5, "the joins add up");
    assert_eq!(cache.stats().coalesced, 5, "the pull channel agrees");

    let text = metrics.render_prometheus();
    assert!(text.contains("# TYPE oxide_cache_plane_coalesce_events_total counter"));
    assert!(text.contains("oxide_cache_plane_coalesce_events_total 2"));
    assert!(text.contains("# TYPE oxide_cache_plane_coalesced_followers_total counter"));
    assert!(text.contains("oxide_cache_plane_coalesced_followers_total 5"));
}

#[tokio::test]
async fn the_feed_carries_the_row_off_the_hot_path() {
    let feed = Arc::new(EventFeed::bounded(8));
    let cache = TieredCache::new(1 << 20).with_observer(feed.clone());
    cache.set("route:/a", entry()).await.unwrap();
    cache.record_coalesced("route:/a", 4);

    let rows = feed.drain();
    let joins: Vec<_> = rows
        .iter()
        .filter(|(_, op)| matches!(op, CacheOp::Coalesce { .. }))
        .collect();
    assert_eq!(joins.len(), 1);
    assert_eq!(joins[0].0.as_str(), "route:/a");
}

#[tokio::test]
async fn the_exporter_render_stays_deterministic_with_the_new_families() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = TieredCache::new(1 << 20).with_observer(metrics.clone());
    cache.record_coalesced("route:/a", 1);
    let one = metrics.render_prometheus();
    let two = metrics.render_prometheus();
    assert_eq!(
        one, two,
        "two renders over the same counters are byte-equal"
    );
    assert!(one.ends_with('\n'));
}
