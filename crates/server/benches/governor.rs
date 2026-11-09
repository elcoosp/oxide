//! Criterion benches for the V2-F12 brownout-mode substrate
//! (`bench-util` gated like the V2-A02/V2-A05/V2-B09/V2-F05/V2-F07
//! benches — plain `cargo test` never pays the criterion build).
//!
//! The wiring-relevant questions:
//! - `next_level`: the sampler tick's pure transition, at the steady-state
//!   shapes a real cadence presents (calm, breaching, the hysteresis band).
//! - `raw_level`: the breach scan itself (the 3-rung × 3-signal worst case).
//! - `governor`: the request path's whole cost — one relaxed atomic load
//!   (the §V2-F12 acceptance contract) and the sampler's store.
//! - `serve_decision`: the enforcement seam per request.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::governor::{
    Governor, Level, Priority, Signals, Thresholds, next_level, raw_level, serve_decision,
};

fn calm() -> Signals {
    Signals {
        queue_wait_p99_us: 1_000,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    }
}

fn breaching(level_us: u64) -> Signals {
    Signals {
        queue_wait_p99_us: level_us,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    }
}

fn bench_next_level(c: &mut Criterion) {
    let t = Thresholds::default();
    let mut group = c.benchmark_group("next_level");
    let mut calm_counter = 0u32;
    group.bench_function("calm_holds_0", |b| {
        b.iter(|| next_level(0, black_box(&calm()), &t, &mut calm_counter))
    });
    let mut shed_counter = 0u32;
    group.bench_function("calm_holds_2_hysteresis", |b| {
        b.iter(|| next_level(2, black_box(&calm()), &t, &mut shed_counter))
    });
    let mut escalate_counter = 0u32;
    group.bench_function("escalate_to_3", |b| {
        b.iter(|| next_level(0, black_box(&breaching(600_000)), &t, &mut escalate_counter))
    });
    let mut band_counter = 0u32;
    group.bench_function("hysteresis_band_holds_2", |b| {
        b.iter(|| next_level(2, black_box(&breaching(150_000)), &t, &mut band_counter))
    });
    group.finish();
}

fn bench_raw_level(c: &mut Criterion) {
    let t = Thresholds::default();
    let mut group = c.benchmark_group("raw_level");
    group.bench_function("all_calm", |b| b.iter(|| raw_level(black_box(&calm()), &t)));
    group.bench_function("level3_breach", |b| {
        b.iter(|| raw_level(black_box(&breaching(600_000)), &t))
    });
    group.finish();
}

fn bench_governor(c: &mut Criterion) {
    let g = Arc::new(Governor::new());
    let mut group = c.benchmark_group("governor");
    // The acceptance contract: the request path's whole brownout cost is
    // this relaxed atomic load.
    group.bench_function("level_read", |b| b.iter(|| g.level()));
    group.bench_function("set", |b| {
        b.iter(|| {
            g.set(black_box(2));
        })
    });
    group.finish();
}

fn bench_serve_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("serve_decision");
    for level in [Level::Normal, Level::Stale, Level::Shed, Level::Emergency] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{level:?}").to_lowercase()),
            &level,
            |b, level| b.iter(|| serve_decision(*level, black_box(Priority::Normal))),
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_next_level,
    bench_raw_level,
    bench_governor,
    bench_serve_decision
);
criterion_main!(benches);
