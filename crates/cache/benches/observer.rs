//! Criterion bench for the cache observability seam (`observer.rs`, §20):
//! the delta an attached [`CacheObserver`](oxide_fw_cache::CacheObserver)
//! puts on the hot memory-hit path against the no-observer default.
//!
//! The wiring-relevant question is the per-request cost of the seam: the
//! pipeline's L0 hit is the single hottest cache path in a serving
//! process, and the no-observer default must stay one `None` branch.
//! The pair pins the truth: same traffic, same subscriber (silent — the
//! rendering cost is not the seam's), the only difference the attached
//! observer. A third case prices the write path (`set` with the tag
//! index updates), the second-warmest path.
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench observer`

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{Entry, Policy, TieredCache, TracingCacheObserver};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;

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

/// The silent subscriber: max level OFF — every layer runs, nothing
/// renders, so the measured delta is the seam's dispatch cost alone.
fn silent_subscriber() {
    let _ = fmt().with_max_level(LevelFilter::OFF).try_init();
}

fn bench_observer_hot_path(c: &mut Criterion) {
    silent_subscriber();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut g = c.benchmark_group("cache_observer");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    let no_observer = Arc::new(TieredCache::new(64 * 1024 * 1024));
    rt.block_on(no_observer.set("route:/", entry()))
        .expect("memory set");
    g.bench_function("fresh_hit_no_observer", |b| {
        b.iter(|| {
            let hit = rt.block_on(no_observer.get(black_box("route:/")));
            black_box(hit.expect("memory get"));
        })
    });

    let with_observer = Arc::new(
        TieredCache::new(64 * 1024 * 1024).with_observer(Arc::new(TracingCacheObserver::new())),
    );
    rt.block_on(with_observer.set("route:/", entry()))
        .expect("memory set");
    g.bench_function("fresh_hit_with_tracing_observer", |b| {
        b.iter(|| {
            let hit = rt.block_on(with_observer.get(black_box("route:/")));
            black_box(hit.expect("memory get"));
        })
    });

    g.bench_function("set_with_tracing_observer", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            let key = format!("route:/{i}");
            rt.block_on(with_observer.set(black_box(&key), entry()))
                .expect("memory set");
        })
    });

    g.finish();
}

criterion_group!(benches, bench_observer_hot_path);
criterion_main!(benches);
