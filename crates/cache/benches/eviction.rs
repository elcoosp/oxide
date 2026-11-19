//! Criterion bench for the cache plane's eviction story (F51/F52, §20):
//! the two costs a host pays for the seam.
//!
//! 1. The prune cost — what a steady-state S-08 eviction adds to the
//!    write path. A zero-budget disk tier prunes the entry it just wrote
//!    on EVERY set, so each benchmarked `set` carries exactly one victim
//!    (victim pick + delete + the two saturating counter adds + the
//!    observer delivery): the honest steady-state shape, with and without
//!    an exporter attached (the F51 event's delivery delta rides the
//!    same call path — one branch + one virtual call, the F28 contract).
//! 2. The read cost — what `stats()` and `health()` add for the eviction
//!    total (one relaxed atomic load over the disk tier's counter, plus
//!    the posture's own sampling pass).

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{DiskTier, Entry, MetricsCacheObserver, Policy, TieredCache};

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

/// The steady-state write: a zero-budget disk tier evicts the entry it
/// just wrote on every set, so one benchmarked set == one completed S-08
/// eviction (victim pick, delete, counter adds, observer delivery).
fn bench_evict_write(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let mut group = c.benchmark_group("cache_evict_write");

    for label in ["no-observer", "with-exporter"] {
        let observer = (label == "with-exporter").then(|| Arc::new(MetricsCacheObserver::new()));
        let disk =
            Arc::new(DiskTier::open_with_budget(dir.path().join(label), 1, 1 << 20).unwrap());
        let mut cache = TieredCache::new(64 * 1024 * 1024).with_disk_tier(Arc::clone(&disk));
        if let Some(obs) = observer {
            cache = cache.with_observer(obs);
        }
        let rt_handle = &rt;
        group.bench_with_input(
            BenchmarkId::new("steady-state-set", label),
            &label,
            |b, _| {
                let mut i = 0u64;
                b.iter(|| {
                    rt_handle.block_on(async {
                        cache
                            .set(
                                black_box(&format!("route:/evict-{i}")),
                                black_box(entry(b"steady-state eviction payload", "blog")),
                            )
                            .await
                            .expect("the zero-budget tier prunes as it writes");
                    });
                    i += 1;
                });
                // The honest post-condition: every set was also one
                // eviction (the counter tells the truth).
                assert_eq!(
                    disk.evictions(),
                    i,
                    "each steady-state set completed exactly one eviction"
                );
            },
        );
    }
    group.finish();
}

/// The pull reads: what `stats()` and `health()` pay for the eviction
/// total — one relaxed load over the disk tier's counter inside the
/// sampling pass each read already runs.
fn bench_evict_reads(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = Arc::new(DiskTier::open_with_budget(dir.path().join("reads"), 1, 1 << 20).unwrap());
    let cache = TieredCache::new(64 * 1024 * 1024).with_disk_tier(disk);
    rt.block_on(async {
        cache
            .set("route:/resident", entry(b"the resident entry", "blog"))
            .await
            .expect("the write lands");
    });
    let mut group = c.benchmark_group("cache_evict_reads");
    group.bench_function("stats/with-disk-tier", |b| {
        b.iter(|| black_box(cache.stats()));
    });
    group.bench_function("health/with-disk-tier", |b| {
        b.iter(|| black_box(cache.health()));
    });
    group.finish();
}

criterion_group!(benches, bench_evict_write, bench_evict_reads);
criterion_main!(benches);
