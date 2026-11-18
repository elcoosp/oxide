//! Criterion bench for the cache plane's metrics exporter (`metrics.rs`,
//! §20): the two costs a host pays for the arm.
//!
//! 1. The per-event counter cost — what an attached
//!    [`MetricsCacheObserver`](oxide_fw_cache::MetricsCacheObserver) adds
//!    to a hot L0 hit beyond the seam dispatch the `cache_observer` bench
//!    already prices: the exporter's arm is one relaxed atomic add for the
//!    layer counter plus one for the served bytes (the `Miss` arm is one
//!    add). Benchmarked with the observer attached and the cache drives
//!    real memory traffic, so the measured number is the full attached
//!    cost, seam dispatch included.
//! 2. The scrape cost — `render_prometheus` over an idle (all-zero) and a
//!    populated exporter. The scrape runs off the hot path (a metrics
//!    scraper polls), so the absolute number matters less than its
//!    stability across the counter state.
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench metrics`

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheEvent, CacheOp, Entry, Layer, MetricsCacheObserver, Policy, TieredCache,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn entry() -> Entry {
    Entry {
        body: vec![0u8; 512],
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec!["content:posts".into()],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // Live wall clock: the benchmarked hit must classify FRESH.
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

/// The attached-traffic case: real memory hits with the exporter on the
/// seam — the per-request cost the serving process pays for the arm.
fn bench_exporter_hot_path(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let metrics = Arc::new(MetricsCacheObserver::new());
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));
    rt.block_on(cache.set("route:/", entry()))
        .expect("memory set");

    let mut g = c.benchmark_group("cache_metrics_exporter");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    g.bench_function("hot-hit/l0/attached", |b| {
        b.iter(|| {
            let hit = rt.block_on(cache.get(black_box("route:/")));
            black_box(hit.expect("memory get"));
        });
    });

    // The direct-event arms price the counter adds without the cache's
    // tier walk — the delta between the miss arm (one add) and the hit
    // arm (two adds + the stale branch).
    g.bench_function("on-event/miss", |b| {
        b.iter(|| {
            let observer: Arc<dyn oxide_fw_cache::CacheObserver> = metrics.clone();
            observer.on_event(&CacheEvent {
                key: black_box("route:/"),
                op: CacheOp::Miss,
            });
        });
    });
    g.bench_function("on-event/hit-l0", |b| {
        b.iter(|| {
            let observer: Arc<dyn oxide_fw_cache::CacheObserver> = metrics.clone();
            observer.on_event(&CacheEvent {
                key: black_box("route:/"),
                op: CacheOp::Hit {
                    layer: Layer::L0,
                    stale: false,
                    bytes: 512,
                },
            });
        });
    });
    // The F33 coalesce arm: one flight row lands as two counter adds
    // (events + joins), the same cost shape as the set arm.
    g.bench_function("on-event/coalesce", |b| {
        b.iter(|| {
            let observer: Arc<dyn oxide_fw_cache::CacheObserver> = metrics.clone();
            observer.on_event(&CacheEvent {
                key: black_box("route:/"),
                op: CacheOp::Coalesce { followers: 3 },
            });
        });
    });

    g.finish();
}

/// The host-side record cost (F33): what
/// `TieredCache::record_coalesced` adds when the flight owner completes —
/// the counter bump plus the row's trip through the seam (exporter
/// attached vs the no-observer default, the seam-dispatch delta the
/// `cache_observer` bench prices for the tier ops). The record is a
/// synchronous counter bump + row delivery — no runtime needed.
fn bench_record_coalesced(c: &mut Criterion) {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let attached = TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone());
    let bare = TieredCache::new(64 * 1024 * 1024);

    let mut g = c.benchmark_group("cache_coalesce_record");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    g.bench_function("attached/exporter", |b| {
        b.iter(|| attached.record_coalesced(black_box("route:/"), 3));
    });
    g.bench_function("no-observer/default", |b| {
        b.iter(|| bare.record_coalesced(black_box("route:/"), 3));
    });

    g.finish();
}

/// The scrape cost: `render_prometheus` over an idle exporter and one
/// carrying traffic — the renderer must be state-stable (the same shape
/// either way; the populated counters only move the numbers).
fn bench_render_scrape(c: &mut Criterion) {
    let idle = MetricsCacheObserver::new();

    let full = MetricsCacheObserver::new();
    let observer: &dyn oxide_fw_cache::CacheObserver = &full;
    for i in 0..64 {
        observer.on_event(&CacheEvent {
            key: "route:/x",
            op: if i % 2 == 0 {
                CacheOp::Hit {
                    layer: Layer::L0,
                    stale: false,
                    bytes: 512,
                }
            } else {
                CacheOp::Miss
            },
        });
    }

    let mut g = c.benchmark_group("cache_metrics_render");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    g.bench_function("scrape/idle", |b| {
        b.iter(|| {
            let text = idle.render_prometheus();
            black_box(text.len());
        });
    });
    g.bench_function("scrape/populated", |b| {
        b.iter(|| {
            let text = full.render_prometheus();
            black_box(text.len());
        });
    });

    g.finish();
}

criterion_group!(
    bench_metrics,
    bench_exporter_hot_path,
    bench_render_scrape,
    bench_record_coalesced
);
criterion_main!(bench_metrics);
