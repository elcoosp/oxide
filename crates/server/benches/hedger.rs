//! V2-F12: the hedger's cost bench — the per-route recent-p95 window's
//! `record`/`hedge_after` arithmetic (the boot's bounded window a hot
//! route's render path pays on every completed render and every
//! eligibility check) and the race seam's arm (the deadline sleep + the
//! gate re-check + the first-completed-wins select the render stage runs).
//!
//! Run with `cargo bench -p oxide-fw-server --features bench-util --bench
//! hedger` (the bench-util opt-in keeps plain `cargo test` off criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxide_fw_server::hedger::{Hedger, hedge_race_when};

/// The window arithmetic: `record` (the post-render write the dispatch
/// wrapper pays per completed render) and `hedge_after` (the eligibility
/// read per render — a cold-window `None`, a warm-window p95 × factor).
fn bench_window(c: &mut Criterion) {
    let h = Arc::new(Hedger::from_config(true, 2.0).expect("the config enables"));
    // Pre-warm two routes: one deep in the window, one cold.
    for i in 0..32u64 {
        h.record("/warm", Duration::from_millis(10 + (i % 5)));
    }
    let mut group = c.benchmark_group("hedger_window");
    group.bench_function("record", |b| {
        b.iter(|| {
            h.record(black_box("/warm"), black_box(Duration::from_millis(12)));
        });
    });
    group.bench_function("hedge_after_warm", |b| {
        b.iter(|| black_box(h.hedge_after(black_box("/warm"))));
    });
    group.bench_function("hedge_after_cold", |b| {
        b.iter(|| black_box(h.hedge_after(black_box("/cold"))));
    });
    group.finish();
}

/// The race seam: the primary always wins cheaply (the gate-closed arm
/// wakes, re-checks, and hangs; the primary resolves the select) — the
/// per-render cost the render stage adds when the deadline never fires,
/// next to the fired shape (the duplicate wins).
fn bench_race(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let started = Arc::new(AtomicUsize::new(0));
    let mut group = c.benchmark_group("hedger_race");
    group.bench_function("gate_closed_primary_wins", |b| {
        b.iter(|| {
            let started = Arc::clone(&started);
            let (value, hedged) = rt.block_on(hedge_race_when(
                async { black_box("primary") },
                move || {
                    started.fetch_add(1, Ordering::SeqCst);
                    async { "secondary" }
                },
                // A saturated pool: the gate holds, no duplicate starts.
                || false,
                black_box(Duration::from_millis(1)),
            ));
            black_box((value, hedged));
        });
    });
    group.bench_function("gate_open_duplicate_wins", |b| {
        b.iter(|| {
            let (value, hedged) = rt.block_on(hedge_race_when(
                async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    black_box("slow-primary")
                },
                || async { black_box("fast-secondary") },
                || true,
                black_box(Duration::from_millis(1)),
            ));
            black_box((value, hedged));
        });
    });
    group.finish();
}

criterion_group!(hedger_benches, bench_window, bench_race);
criterion_main!(hedger_benches);
