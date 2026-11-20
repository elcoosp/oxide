//! Criterion bench for the client's peer-posture probes (F63): the cost
//! a wired node's posture question pays over the wire — the `peer_health`
//! pull (connect + request + the F58 JSON decode) and the `peer_metrics`
//! pull (connect + request + the §20 exposition read) against a REAL
//! plane service on a local socket, beside the refused-dial floor the
//! degradation arms ride.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench peer_probe`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, HttpCacheHandler, HttpCacheService, MetricsCacheObserver, MetricsPlaneSource,
    PlaneHealth, PlaneMetrics, PlanePeer, TieredCache,
};

fn bench_peer_probe(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // The peer: the full plane on a real socket — the boot's own
    // construction (the exporter fanned onto the tier, the tier rooted as
    // the health provider, both composed into the metrics source).
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    );
    let source = MetricsPlaneSource::new(
        metrics.clone(),
        tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
    );
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
        .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>)
        .with_metrics(Arc::new(source) as Arc<dyn PlaneMetrics>);
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("the bench listener binds");
    let addr = listener.local_addr().expect("the bound address");
    rt.spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"));

    // The refused floor: bind, drop, dial — the transport-degrade arm's
    // cost (the connect error the probes propagate under `CACHE-HTTP`).
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("the floor listener binds")
        .local_addr()
        .expect("the floor address");
    let refused = HttpCacheHandler::new(format!("http://{dead}"));

    let mut group = c.benchmark_group("peer_probe");
    group.throughput(criterion::Throughput::Elements(1));

    // The posture pull: one full exchange + the serde decode — the cost
    // an operator probe (or a wired node's readiness check) pays per ask.
    group.bench_function(BenchmarkId::new("health", 200), |b| {
        b.iter(|| {
            let health = rt.block_on(client.peer_health()).expect("the pull answers");
            black_box(health.memory_entries);
        });
    });

    // The exposition pull: one full exchange + the §20 text read — the
    // cost a scraper pointing THROUGH the client pays per scrape.
    group.bench_function(BenchmarkId::new("metrics", 200), |b| {
        b.iter(|| {
            let text = rt
                .block_on(client.peer_metrics())
                .expect("the pull answers");
            black_box(text.len());
        });
    });

    // The degrade floor: a refused dial — the arm the §11.2 "a remote
    // cache is an optimization" contract rides when the peer is gone.
    group.bench_function(BenchmarkId::new("refused-err", 0), |b| {
        b.iter(|| {
            let outcome = rt.block_on(refused.peer_health());
            black_box(outcome.is_err());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_peer_probe);
criterion_main!(benches);
