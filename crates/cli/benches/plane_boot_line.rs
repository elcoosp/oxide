//! V2-F63 recorded pickup: the boot posture line's cost benches — the
//! line's two halves, measured on this machine, not assumed: the
//! byte-stable render over a fixed posture (the boot pays it once per
//! `cache.handler = { type: 'http' }` boot) and the bounded probe +
//! render over a LIVE plane (the pull the serve loop rides, the F60
//! deadline attached).
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! plane_boot_line` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;

use oxide_fw_cache::{
    CacheHandler, CacheHealth, CacheObserver, HttpCacheService, MetricsCacheObserver,
    MetricsPlaneSource, PlaneHealth, PlaneMetrics, TieredCache,
};
use oxide_fw_cli::{PLANE_BOOT_PROBE_DEADLINE, remote_plane_boot_line, render_plane_boot_line};

/// The fixed posture the render arm rides — the fields the F58 pull
/// reports, filled to a mid-size plane's shape.
fn posture() -> CacheHealth {
    CacheHealth {
        memory_entries: 128,
        memory_bytes: 5 * 1024 * 1024,
        memory_capacity_bytes: 64 * 1024 * 1024,
        disk_attached: true,
        disk_bytes: 16 * 1024 * 1024,
        remote_attached: false,
        remote_kind: None,
        tags_indexed: 42,
        keys_tracked: 1307,
        observer_attached: true,
        disk_evictions: 3,
    }
}

/// A live plane on a real socket — the boot's own construction: the tier
/// rooted as both the backing store and the F58 health provider, the §20
/// exporter fanned onto it and composed into the F61 metrics source.
fn spawn_plane(rt: &tokio::runtime::Runtime) -> std::net::SocketAddr {
    rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let metrics = Arc::new(MetricsCacheObserver::new());
        let tier = Arc::new(
            TieredCache::new(64 * 1024 * 1024)
                .with_observer(metrics.clone() as Arc<dyn CacheObserver>),
        );
        let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>)
            .with_metrics(Arc::new(MetricsPlaneSource::new(
                metrics,
                tier.clone() as Arc<dyn PlaneHealth>,
            )) as Arc<dyn PlaneMetrics>);
        tokio::spawn(async move {
            svc.serve(l).await.expect("the accept loop serves");
        });
        addr
    })
}

/// The render half: the byte-stable line body over a fixed posture. Two
/// renders over the same posture are byte-equal — the cost is the
/// format, not a pull.
fn bench_plane_boot_line_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("plane_boot_line_render");
    let health = posture();
    group.bench_function("fixed_posture", |b| {
        b.iter(|| black_box(render_plane_boot_line(black_box(&health))));
    });
    group.finish();
}

/// The probe half: the bounded pull over the LIVE plane, then the render
/// — the full line a `cache.handler = { type: 'http' }` boot prints.
fn bench_plane_boot_line_probe(c: &mut Criterion) {
    let mut group = c.benchmark_group("plane_boot_line_probe");
    let rt = tokio::runtime::Runtime::new().expect("the bench's tokio runtime");
    let addr = spawn_plane(&rt);
    let url = format!("http://{addr}");
    group.bench_function("live_pull_render", |b| {
        b.iter(|| {
            let line = rt.block_on(remote_plane_boot_line(
                black_box(&url),
                PLANE_BOOT_PROBE_DEADLINE,
            ));
            black_box(line.expect("the live plane answers"));
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_plane_boot_line_render,
    bench_plane_boot_line_probe
);
criterion_main!(benches);
