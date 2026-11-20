//! Criterion bench for the cache plane's metrics arm (F61): the full
//! scrape cost `GET /metrics` pays through the service's routing core —
//! both §20 halves (the counter pull + render, the gauge pull + render) —
//! against the unconfigured 404 floor (the F40 None-check) and the
//! render-only shape a host's own in-process scrape rides.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench plane_metrics`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    HttpCacheService, MetricsCacheObserver, MetricsPlaneSource, PlaneRequest, TieredCache,
};

fn bench_metrics(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    );
    let source = MetricsPlaneSource::new(
        metrics.clone(),
        tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
    );
    let wired = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_metrics(Arc::new(source) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    let plain = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);

    let mut group = c.benchmark_group("cache_plane_metrics");
    group.throughput(criterion::Throughput::Elements(1));

    // The configured arm: both pulls and the joined render — the cost a
    // collector's scrape pays through the plane's own wire.
    group.bench_function(BenchmarkId::new("pull", 200), |b| {
        b.iter(|| {
            let resp = rt.block_on(wired.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/metrics".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
            black_box(resp.body.len());
        });
    });

    // The render-only shape: the observer's own in-process scrape — the
    // pull is the same snapshot the wire arm serves, minus the routing.
    group.bench_function(BenchmarkId::new("render-only", 0), |b| {
        b.iter(|| {
            let text = metrics.render_prometheus();
            black_box(text.len());
        });
    });

    // The unconfigured floor: the pre-F61 shape — one None check, the
    // route's `Unknown` 404. The F40 doctrine's cost, zero.
    group.bench_function(BenchmarkId::new("unconfigured-404", 404), |b| {
        b.iter(|| {
            let resp = rt.block_on(plain.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/metrics".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_metrics);
criterion_main!(benches);
