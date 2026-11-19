//! Criterion bench for the cache plane's posture pull (`health.rs`, §20):
//! the two costs a host pays for the seam.
//!
//! 1. The snapshot cost — what `TieredCache::health()` adds per probe:
//!    the memory tier's `len`/`weighted_size` (both flush moka's pending
//!    maintenance, the exact-metrics contract), the disk tier's atomic
//!    byte counter, the tag/reverse-index map lengths, and the
//!    introspection call over the opaque remote. Benchmarked memory-only
//!    (the boot probe's cheapest stack) and full-stack (disk + remote +
//!    populated tag index — the serving process's steady posture), so a
//!    probe cadence can be priced against both.
//! 2. The render cost — `render_prometheus` over an idle (all-zero) and a
//!    populated posture. The render runs off the hot path (a scraper or
//!    the health endpoint polls), so the absolute number matters less
//!    than its stability across the posture state.
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench health`

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{CacheHealth, Entry, Policy, TieredCache};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn entry(body: &[u8], tag: &str) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![tag.into()],
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

fn bench_health_snapshot(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let dir = tempfile::tempdir().expect("tempdir");

    // The cheapest stack: memory only, nothing resident — the boot probe.
    let memory_only = TieredCache::new(64 * 1024 * 1024);

    // The steady stack: disk tier + remote tier + a populated tag index.
    let remote: Arc<dyn oxide_fw_cache::CacheHandler> = TieredCache::new_memory();
    let full = TieredCache::new(64 * 1024 * 1024)
        .with_disk(dir.path().join("bench-tier"))
        .expect("disk tier")
        .with_remote(remote)
        .with_observer(Arc::new(oxide_fw_cache::EventFeed::bounded(64)));
    rt.block_on(async {
        for i in 0..64 {
            full.set(
                &format!("route:/page-{i}"),
                entry(&[0u8; 512], "content:posts"),
            )
            .await
            .expect("memory set");
        }
    });

    let mut g = c.benchmark_group("cache_health_snapshot");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    g.bench_function("memory-only/idle", |b| {
        b.iter(|| black_box(memory_only.health()));
    });
    g.bench_function("full-stack/populated", |b| {
        b.iter(|| black_box(full.health()));
    });

    g.finish();
}

fn bench_health_render(c: &mut Criterion) {
    let idle = TieredCache::new(64 * 1024 * 1024).health();
    // A populated posture without the async traffic — the render arm
    // prices the string building, not the snapshot walk.
    let populated = CacheHealth {
        memory_entries: 64,
        memory_bytes: 64 * (512 + 21 + 32),
        memory_capacity_bytes: 64 * 1024 * 1024,
        disk_attached: true,
        disk_bytes: 64 * (512 + 21 + 32 + 16),
        remote_attached: true,
        remote_kind: Some("http".into()),
        tags_indexed: 3,
        keys_tracked: 64,
        observer_attached: true,
        // F52: the populated arm prices the appended gauge's row too.
        disk_evictions: 6,
    };

    let mut g = c.benchmark_group("cache_health_render");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(3));

    g.bench_function("scrape/idle", |b| {
        b.iter(|| black_box(&idle).render_prometheus());
    });
    g.bench_function("scrape/populated", |b| {
        b.iter(|| black_box(&populated).render_prometheus());
    });

    g.finish();
}

criterion_group!(benches, bench_health_snapshot, bench_health_render);
criterion_main!(benches);
