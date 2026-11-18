//! Criterion bench for the cache plane's observer fanout (`fanout.rs`,
//! §20): what the composition adds to the two sides the seam's benches
//! already measure.
//!
//! 1. The per-event fan delta — `on_event` over one child versus two
//!    versus three: the fan adds one virtual call per child (and zero
//!    copies — the event borrow is re-shared), so the delta is the
//!    children's own cost.
//! 2. The hot path — a fresh L0 hit with a three-child fan attached
//!    (tracing + metrics + feed, the dashboard boot's attach shape)
//!    versus the observer bench's single-observer arms: the composed
//!    boot's whole per-hit observability cost in one number.
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench fanout`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheEvent, CacheObserver, CacheOp, Entry, FanoutCacheObserver, MetricsCacheObserver, Policy,
    TieredCache,
};

fn entry(body: &[u8]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
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

fn fan_of(children: Vec<Arc<dyn CacheObserver>>) -> Arc<FanoutCacheObserver> {
    FanoutCacheObserver::shared(children)
}

fn bench_fan_delta(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_observer_fanout");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    let event = CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 512 },
    };

    // One child: the fan's own branch plus the child's cost.
    let one = fan_of(vec![Arc::new(MetricsCacheObserver::new())]);
    g.bench_function("on-event/one-child", |b| {
        b.iter(|| one.on_event(black_box(&event)));
    });

    // Two children: the exporter + the feed — the composite the `coalesce`
    // walkthrough hand-rolled before the fan shipped.
    let two = fan_of(vec![
        Arc::new(MetricsCacheObserver::new()) as Arc<dyn CacheObserver>,
        Arc::new(oxide_fw_cache::EventFeed::bounded(1024)) as Arc<dyn CacheObserver>,
    ]);
    g.bench_function("on-event/two-children", |b| {
        b.iter(|| two.on_event(black_box(&event)));
    });

    // Three children: tracing + exporter + feed — the dashboard boot's
    // attach shape (the tracing arm renders nothing here: no subscriber
    // installed, the `tracing` contract's no-op).
    let three = fan_of(vec![
        Arc::new(oxide_fw_cache::TracingCacheObserver::new()) as Arc<dyn CacheObserver>,
        Arc::new(MetricsCacheObserver::new()) as Arc<dyn CacheObserver>,
        Arc::new(oxide_fw_cache::EventFeed::bounded(1024)) as Arc<dyn CacheObserver>,
    ]);
    g.bench_function("on-event/three-children", |b| {
        b.iter(|| three.on_event(black_box(&event)));
    });

    g.finish();
}

fn bench_fan_hot_path(c: &mut Criterion) {
    let mut g = c.benchmark_group("cache_observer_fanout");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    let cache = Arc::new(
        TieredCache::new(64 * 1024 * 1024).with_observer(fan_of(vec![
            Arc::new(oxide_fw_cache::TracingCacheObserver::new()) as Arc<dyn CacheObserver>,
            Arc::new(MetricsCacheObserver::new()) as Arc<dyn CacheObserver>,
            Arc::new(oxide_fw_cache::EventFeed::bounded(1024)) as Arc<dyn CacheObserver>,
        ])),
    );
    let tokio = tokio::runtime::Runtime::new().expect("the bench runtime");
    tokio
        .block_on(cache.set("route:/a", entry(b"hello")))
        .expect("the memory-tier set completes");

    g.bench_function("fresh-hit/fanout-attached", |b| {
        b.iter(|| {
            tokio.block_on(async {
                let hit = cache.get("route:/a").await.expect("the memory tier");
                black_box(hit.is_some());
            });
        });
    });

    g.finish();
}

criterion_group!(bench_fanout, bench_fan_delta, bench_fan_hot_path);
criterion_main!(bench_fanout);
