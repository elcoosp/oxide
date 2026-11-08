//! V2-B09: criterion benchmarks for cache keys, tag indexing and disk writes
//! (`crates/cache`).
//!
//! Cases (spec V2-B09): key construction (old `canonical()` and the V2-B05
//! `key_of` stack key), tag insert at a 10k-key tag, and a disk-tier `set`
//! carrying 5 tags. Bench ids stay stable across Wave B WPs so
//! `benchmarks/criterion_compare.py` can diff against `baseline-v01.json`.
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{DiskTier, Entry, Policy, TieredCache};
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn entry(tags: &[&str]) -> Entry {
    Entry {
        body: vec![0u8; 512],
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
        etag: "v1".into(),
    }
}

fn bench_key_construction(c: &mut Criterion) {
    let mut g = c.benchmark_group("key_construction");
    g.bench_function("canonical", |b| {
        b.iter(|| {
            black_box(oxide_fw_cache::CacheKey::canonical_parts(
                oxide_fw_cache::Layer::L0,
                "routes/blog/[slug]/page",
                r#"{"slug":"hello-world"}"#,
                "accept-encoding",
                "build-16",
            ))
        })
    });
    // V2-B05: the stack-allocated 128-bit key. Present from B05 on; the
    // compare script only diffs keys present in both files.
    g.bench_function("key_of", |b| {
        b.iter(|| {
            black_box(oxide_fw_cache::key_of(
                "L0",
                "routes/blog/[slug]/page",
                r#"{"slug":"hello-world"}"#,
                "accept-encoding",
                "build-16",
            ))
        })
    });
    g.finish();
}

/// Preload `n` keys under one tag, then measure the amortized cost of further
/// inserts into that same tag (the P-18/B05 contract: O(1), not O(n)).
fn bench_tag_insert(c: &mut Criterion) {
    let mut g = c.benchmark_group("tag_insert");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    for n in [10_000usize] {
        let cache = TieredCache::new_memory();
        rt.block_on(async {
            for i in 0..n {
                let e = entry(&["hot-tag"]);
                cache
                    .set(&format!("routes/{i}/page"), e)
                    .await
                    .expect("seed set");
            }
        });
        g.bench_with_input(BenchmarkId::from_parameter(n), &cache, |b, cache| {
            b.iter(|| {
                rt.block_on(async {
                    for i in 0..100 {
                        let e = entry(&["hot-tag"]);
                        let _ = cache
                            .set(&format!("insert/{i}"), black_box(e.clone()))
                            .await;
                    }
                })
            })
        });
    }
    g.finish();
}

fn bench_disk_set(c: &mut Criterion) {
    let mut g = c.benchmark_group("disk_set");
    g.sample_size(20);
    g.measurement_time(Duration::from_secs(4));
    let tmp = tempfile::tempdir().expect("tempdir");
    let disk = Arc::new(DiskTier::open(tmp.path().join("cache")).expect("disk tier"));
    g.bench_function("5_tags", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            let e = entry(&["t1", "t2", "t3", "t4", "t5"]);
            disk.set(&format!("key/{i}"), black_box(&e))
                .expect("disk set");
        })
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_key_construction,
    bench_tag_insert,
    bench_disk_set
);
criterion_main!(benches);
