//! TDD suite for the cache plane's metrics exporter (`metrics.rs`). Written
//! first per the house TDD rule: every pin failed on unresolved symbols
//! (E0432 over `MetricsCacheObserver`/`CacheMetricsSnapshot`, E0599 over
//! `render_prometheus`) before the exporter existed.
//!
//! The contract the pins hold:
//!
//! 1. [`MetricsCacheObserver`](oxide_fw_cache::MetricsCacheObserver) is the
//!    F28 seam's metrics arm — the consumer the observer module's doc names
//!    ("attach a custom impl for the structured end — a metrics exporter").
//!    It implements [`CacheObserver`](oxide_fw_cache::CacheObserver) and
//!    rides the same attach point (`TieredCache::with_observer`, the
//!    `Arc<dyn CacheObserver>` coercion).
//! 2. Every completed op lands in one atomic counter per family: per-tier
//!    hits (the serving layer), stale hits (the SWR window), misses, sets,
//!    deletes, bytes served/stored, and the aggregate invalidation rows
//!    (one row, not one per key — the `CacheOp::Invalidate` contract).
//! 3. [`CacheMetricsSnapshot`](oxide_fw_cache::CacheMetricsSnapshot) is the
//!    value-shaped pull mirror (`Clone` + `Eq` + `Default`), the
//!    `CountingObserver::captured` posture — tests and hosts compare
//!    values, not pointers.
//! 4. `render_prometheus` is the §20 text exposition: `# HELP`/`# TYPE`
//!    counter pairs, the server crate's `Metrics::render` shape, a
//!    deterministic row order, a trailing newline — and the
//!    `oxide_cache_plane_*` family name, so the request plane's
//!    `oxide_cache_l0_*` page-render rows and the §11.2 cache plane's rows
//!    stay two distinct families on one scrape.
//! 5. Zero traffic renders zero rows: a fresh observer's exposition is the
//!    all-zero scrape a dashboard registers before the first request.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxide_fw_cache::{
    CacheEvent, CacheMetricsSnapshot, CacheObserver, CacheOp, Entry, Layer, MetricsCacheObserver,
    Policy, TieredCache,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Drive one event through the seam the way the cache does — the
/// `&dyn CacheObserver` object form the tiers' field stores.
fn send(observer: &dyn CacheObserver, key: &str, op: CacheOp) {
    observer.on_event(&CacheEvent { key, op });
}

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|s| s.to_string()).collect(),
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

// --------------------------------------------------------------- the trait pin

/// The seam pin: the exporter IS a `CacheObserver`, attachable through the
/// F28 builder (the `Arc<dyn CacheObserver>` coercion the tiers' field
/// stores).
#[test]
fn metrics_observer_is_a_cache_observer() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));
    // No panic = the coercion + attach hold; the traffic pins below prove
    // the rows land.
    assert!(cache.stats().hits == 0);
    let _ = &*metrics;
}

// ------------------------------------------------------------ the counter pins

/// Real traffic through the attached cache: every completed op lands in its
/// counter, and the exporter's totals agree with the pull channel's own
/// `CacheStats` where the two overlap.
#[tokio::test]
async fn counters_track_real_traffic() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));

    cache
        .set("route:/a", entry(b"hello", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/missing").await.expect("the memory tier");
    cache.delete("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/a").await.expect("the memory tier");

    let snap = metrics.snapshot();
    assert_eq!(snap.sets, 1, "one write-through completion");
    assert_eq!(snap.hits_l0, 1, "the first get is a fresh L0 hit");
    assert_eq!(snap.misses, 2, "the missing key and the deleted key");
    assert_eq!(snap.deletes, 1);
    assert_eq!(snap.served_bytes, 5, "the hit served the entry body");
    assert_eq!(snap.stored_bytes, 5, "the set stored the entry body");
    assert_eq!(snap.hits_l1, 0, "a memory-only cache never serves L1");
    assert_eq!(snap.hits_l2, 0);
    assert_eq!(snap.stale_hits, 0, "the entry is fresh inside its ttl");

    // Pull/push parity on the overlapping totals (the stats contract).
    let stats = cache.stats();
    assert_eq!(stats.hits, snap.hits_l0 + snap.hits_l1 + snap.hits_l2);
    assert_eq!(stats.misses, snap.misses);
}

/// Direct-event pins over the arms real traffic cannot reach in a
/// memory-only cache: the L1/L2 serving layers and the stale flag (the SWR
/// window's serve).
#[test]
fn per_layer_hits_and_the_stale_flag() {
    let metrics = MetricsCacheObserver::new();

    send(
        &metrics,
        "route:/l0",
        CacheOp::Hit {
            layer: Layer::L0,
            stale: false,
            bytes: 10,
        },
    );
    send(
        &metrics,
        "route:/l1",
        CacheOp::Hit {
            layer: Layer::L1,
            stale: false,
            bytes: 20,
        },
    );
    send(
        &metrics,
        "route:/l2",
        CacheOp::Hit {
            layer: Layer::L2,
            stale: true,
            bytes: 30,
        },
    );

    let snap = metrics.snapshot();
    assert_eq!(snap.hits_l0, 1);
    assert_eq!(snap.hits_l1, 1);
    assert_eq!(snap.hits_l2, 1);
    assert_eq!(snap.stale_hits, 1, "the SWR-window serve");
    assert_eq!(snap.served_bytes, 60, "the bodies ride the rows");
    assert_eq!(snap.misses, 0);
}

/// The aggregate invalidation row: one event, the affected-key count rides
/// it — never one event per key (the `CacheOp::Invalidate` contract), and
/// the exporter keeps both halves (rows + keys).
#[tokio::test]
async fn invalidate_aggregates_rows_and_keys() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));

    cache
        .set("route:/a", entry(b"a", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");
    cache
        .set("route:/b", entry(b"b", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");

    let n = cache
        .invalidate_tag("content:posts", false)
        .await
        .expect("the tag index");
    assert_eq!(n, 2, "two distinct keys affected");

    let snap = metrics.snapshot();
    assert_eq!(snap.invalidations, 1, "one aggregate row, not two");
    assert_eq!(snap.invalidated_keys, 2, "the count the call returned");
}

/// The record shape a filter or feed keys on rides through unchanged: the
/// exporter consumes the op payload and never fabricates a row of its own
/// (a `Miss` on one key, an `Invalidate` row, then the delta).
#[test]
fn record_shaped_events_drive_the_deltas() {
    let metrics = MetricsCacheObserver::new();

    send(&metrics, "route:/a", CacheOp::Miss);
    send(&metrics, "route:/a", CacheOp::Set { bytes: 128 });
    send(&metrics, "route:/a", CacheOp::Delete);
    send(
        &metrics,
        "route:/a",
        CacheOp::Invalidate {
            tag: "content:posts".into(),
            soft: true,
            count: 7,
        },
    );

    let snap = metrics.snapshot();
    assert_eq!(snap.misses, 1);
    assert_eq!(snap.sets, 1);
    assert_eq!(snap.stored_bytes, 128);
    assert_eq!(snap.deletes, 1);
    assert_eq!(snap.invalidations, 1);
    assert_eq!(snap.invalidated_keys, 7);
    assert_eq!(snap.hits_l0 + snap.hits_l1 + snap.hits_l2, 0);
    assert_eq!(snap.served_bytes, 0);
}

// ------------------------------------------------------------ the snapshot pin

/// The value mirror: `Default` is the all-zero scrape, `Clone` + `Eq` hold
/// (the `CountingObserver::captured` posture — compare values, not
/// pointers).
#[test]
fn snapshot_is_a_value_type() {
    let zero = CacheMetricsSnapshot::default();
    assert_eq!(
        zero,
        CacheMetricsSnapshot {
            hits_l0: 0,
            hits_l1: 0,
            hits_l2: 0,
            stale_hits: 0,
            misses: 0,
            sets: 0,
            deletes: 0,
            served_bytes: 0,
            stored_bytes: 0,
            invalidations: 0,
            invalidated_keys: 0,
            coalesce_events: 0,
            coalesced_followers: 0,
            evictions: 0,
            evicted_bytes: 0,
        }
    );

    let metrics = MetricsCacheObserver::new();
    send(&metrics, "k", CacheOp::Set { bytes: 4 });
    let first = metrics.snapshot();
    assert_eq!(first, first.clone(), "Clone + Eq");
    assert_ne!(first, zero, "the event moved the mirror");
}

// ----------------------------------------------------------- the render pins

/// The §20 text exposition's shape: every family renders one `# HELP`/`# TYPE`
/// counter pair, the hits family splits per serving layer (the fixed
/// `L0`/`L1`/`L2` label set), the row order is deterministic, and the
/// exposition ends with a newline.
#[test]
fn render_prometheus_row_shape() {
    let metrics = MetricsCacheObserver::new();
    let out = metrics.render_prometheus();

    for family in [
        "oxide_cache_plane_hits_total",
        "oxide_cache_plane_stale_hits_total",
        "oxide_cache_plane_misses_total",
        "oxide_cache_plane_sets_total",
        "oxide_cache_plane_served_bytes_total",
        "oxide_cache_plane_stored_bytes_total",
        "oxide_cache_plane_deletes_total",
        "oxide_cache_plane_invalidations_total",
        "oxide_cache_plane_invalidated_keys_total",
    ] {
        assert!(
            out.contains(&format!("# HELP {family} ")),
            "missing HELP for {family}"
        );
        assert!(
            out.contains(&format!("# TYPE {family} counter\n")),
            "missing counter TYPE for {family}"
        );
    }

    for layer in ["L0", "L1", "L2"] {
        assert!(
            out.contains(&format!(
                "oxide_cache_plane_hits_total{{layer=\"{layer}\"}} 0\n"
            )),
            "missing {layer} hits row"
        );
    }

    assert!(out.ends_with('\n'), "the exposition ends with a newline");
    assert_eq!(out, metrics.render_prometheus(), "byte-stable re-renders");
}

/// The render reflects the counters: after a few events the values line up
/// with the snapshot (one renderer, one source of truth).
#[test]
fn render_reflects_the_counters() {
    let metrics = MetricsCacheObserver::new();
    send(
        &metrics,
        "route:/a",
        CacheOp::Hit {
            layer: Layer::L1,
            stale: false,
            bytes: 42,
        },
    );
    send(&metrics, "route:/a", CacheOp::Set { bytes: 9 });

    let out = metrics.render_prometheus();
    assert!(
        out.contains("oxide_cache_plane_hits_total{layer=\"L1\"} 1\n"),
        "the L1 hit renders"
    );
    assert!(
        out.contains("oxide_cache_plane_sets_total 1\n"),
        "the set renders"
    );
    assert!(
        out.contains("oxide_cache_plane_served_bytes_total 42\n"),
        "the served body renders"
    );
    assert!(
        out.contains("oxide_cache_plane_stored_bytes_total 9\n"),
        "the stored body renders"
    );
    let snap = metrics.snapshot();
    assert_eq!(snap.hits_l1, 1);
    assert_eq!(snap.sets, 1);
}
