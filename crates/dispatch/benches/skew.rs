//! Criterion bench for the §V2-F10 skew-protection pool — the per-request
//! cost of the build-id check as the pool depth grows.
//!
//! The shapes: the §V2-A04 1-deep `GenerationSet` (the baseline the stage
//! has always paid), the bounded pool at the F10 default-ish depth (8
//! demoted generations) with the current id (the hot path every fresh
//! request takes), with the OLDEST demoted id (the worst-case pool walk)
//! and a miss (the 409 path). The stage itself is unchanged — the bench
//! measures the `accepts` lookup behind it.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::build::GenerationSet;
use oxide_fw_dispatch::skew::{AcceptedGenerations, GenerationPool};

const WINDOW: Duration = Duration::from_mins(15);

/// A fixed request instant: pool demotions at one-second offsets, so the
/// `Instant` arithmetic is identical across iterations.
fn bench_pool(c: &mut Criterion) {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(120);

    let mut set = GenerationSet::new("0123456789abcdef".to_string(), WINDOW);
    set.promote("2222222222222222".to_string(), t0);

    let mut pool = GenerationPool::new("0123456789abcdef".to_string(), WINDOW, 8);
    for i in 1..=8u64 {
        pool.promote(format!("{i:016x}"), t0 + Duration::from_secs(i));
    }

    let mut group = c.benchmark_group("skew");
    group.bench_function("stage_set_current", |b| {
        b.iter(|| {
            black_box(oxide_fw_dispatch::skew::stage_check(
                "/__data",
                &[("x-oxide-build".to_string(), "2222222222222222".to_string())],
                black_box(&set),
                now,
            ));
        });
    });
    group.bench_function(BenchmarkId::new("stage_pool_current", pool.len()), |b| {
        b.iter(|| {
            black_box(oxide_fw_dispatch::skew::stage_check(
                "/__data",
                &[("x-oxide-build".to_string(), "0123456789abcdef".to_string())],
                black_box(&pool),
                now,
            ));
        });
    });
    group.bench_function(
        BenchmarkId::new("stage_pool_oldest_demoted", pool.len()),
        |b| {
            b.iter(|| {
                black_box(oxide_fw_dispatch::skew::stage_check(
                    "/__data",
                    &[("x-oxide-build".to_string(), "0000000000000001".to_string())],
                    black_box(&pool),
                    now,
                ));
            });
        },
    );
    group.bench_function(BenchmarkId::new("stage_pool_miss", pool.len()), |b| {
        b.iter(|| {
            black_box(oxide_fw_dispatch::skew::stage_check(
                "/__data",
                &[("x-oxide-build".to_string(), "9999999999999999".to_string())],
                black_box(&pool),
                now,
            ));
        });
    });
    // The trait object story (the endpoint may hold `dyn
    // AcceptedGenerations` when the source is chosen at boot): one
    // dynamic dispatch on the same hot path.
    let dyn_pool: &dyn AcceptedGenerations = &pool;
    group.bench_function(
        BenchmarkId::new("stage_pool_dyn_current", pool.len()),
        |b| {
            b.iter(|| {
                black_box(oxide_fw_dispatch::skew::stage_check(
                    "/__data",
                    &[("x-oxide-build".to_string(), "0123456789abcdef".to_string())],
                    black_box(dyn_pool),
                    now,
                ));
            });
        },
    );
    group.finish();
}

criterion_group!(skew_benches, bench_pool);
criterion_main!(skew_benches);
