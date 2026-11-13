//! Criterion bench for the §V2-F10 serving pools — the per-request cost of
//! the build-id ROUTING (which pool serves this id) as the pool depth
//! grows, on top of the `skew` group's acceptance-check shapes.
//!
//! The shapes: the absent id (every HTML navigation and header-less
//! data-plane request — the "always current" fast path), the current id
//! (the fresh-tab hot path), the OLDEST demoted id at depth 8 (the
//! worst-case walk), a miss (the `None` the 409 path rides) and the
//! stage-through-the-seam shape (the §V2-A04 stage driving the pools —
//! the same dynamic-dispatch question the `skew` group asks of the id-only
//! pool). Demotions sit at one-second offsets so the `Instant` arithmetic
//! is identical across iterations.

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::pools::GenerationPools;

const WINDOW: Duration = Duration::from_mins(15);
const DEPTH: usize = 8;

/// The fake serving handle: the bench measures the routing decision, not
/// what the pool does.
struct BenchPool;

fn bench_pools(c: &mut Criterion) {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(120);

    let mut pools = GenerationPools::new(
        "0000000000000000".to_string(),
        Arc::new(BenchPool),
        WINDOW,
        DEPTH,
    );
    // Eight deploys at one-second offsets: after the loop the current id
    // is 0000000000000008 and the demoted deque holds
    // 0000000000000000..0000000000000007, oldest first.
    for i in 1..=DEPTH as u64 {
        pools.promote(
            format!("{i:016x}"),
            Arc::new(BenchPool),
            t0 + Duration::from_secs(i),
        );
    }
    let current = format!("{DEPTH:016x}");
    let oldest = "0000000000000000".to_string();

    let mut group = c.benchmark_group("pools");
    group.bench_function("route_absent_id_html", |b| {
        b.iter(|| {
            black_box(pools.route(None, now));
        });
    });
    group.bench_function(BenchmarkId::new("route_current", pools.len()), |b| {
        b.iter(|| {
            black_box(pools.route(Some(black_box(&current)), now));
        });
    });
    group.bench_function(BenchmarkId::new("route_oldest_demoted", pools.len()), |b| {
        b.iter(|| {
            black_box(pools.route(Some(black_box(&oldest)), now));
        });
    });
    group.bench_function(BenchmarkId::new("route_miss", pools.len()), |b| {
        b.iter(|| {
            black_box(pools.route(Some(black_box("9999999999999999")), now));
        });
    });
    // The stage drives the pools through the AcceptedGenerations seam —
    // the same verdict question the `skew` group asks of the id-only
    // pool, now over the serving structure.
    group.bench_function(BenchmarkId::new("stage_pools_current", pools.len()), |b| {
        b.iter(|| {
            black_box(oxide_fw_dispatch::skew::stage_check(
                "/__data",
                &[(
                    oxide_fw_dispatch::build::BUILD_HEADER.to_string(),
                    current.clone(),
                )],
                black_box(&pools),
                now,
            ));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_pools);
criterion_main!(benches);
