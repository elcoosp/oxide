//! Criterion bench for the cache plane's health arm (F58): the per-pull
//! cost an operator's probe pays for `GET /health` through the service's
//! routing core, against the unconfigured 404 floor (the F40 None-check).
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench plane_health`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{HttpCacheService, PlaneRequest, TieredCache};

fn bench_health(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    let tier = TieredCache::new_memory();
    let provider: Arc<dyn oxide_fw_cache::PlaneHealth> = tier.clone();
    let healthy = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_health(provider);
    let plain = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);

    let mut group = c.benchmark_group("cache_plane_health");
    group.throughput(criterion::Throughput::Elements(1));

    // The configured arm: the health pull — one CacheHealth snapshot
    // (serde) per scrape, the cost a probe's poll pays.
    group.bench_function(BenchmarkId::new("pull", 200), |b| {
        b.iter(|| {
            let resp = rt.block_on(healthy.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/health".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
            black_box(resp.body.len());
        });
    });

    // The unconfigured floor: the pre-F58 shape — one None check, the
    // route's `Unknown` 404. The F40 doctrine's cost, zero.
    group.bench_function(BenchmarkId::new("unconfigured-404", 404), |b| {
        b.iter(|| {
            let resp = rt.block_on(plain.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/health".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_health);
criterion_main!(benches);
