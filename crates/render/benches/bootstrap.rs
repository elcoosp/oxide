//! Criterion bench for the hydration-bootstrap emitters (V2-F03).
//!
//! The wiring-relevant question is the per-page cost of the bootstrap
//! emission the injection pass runs on every rendered route: the
//! pre-F03 shapes (`with_build` / `with_live`) versus the
//! replay-advertising one (`with_replay` with the flag off — the only
//! spelling that changes bytes) and with the flag on (the delegation
//! path that must stay byte-identical).

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_render::{
    hydration_bootstrap_with_build, hydration_bootstrap_with_live, hydration_bootstrap_with_replay,
};

const B64: &str = "eyJkIjoxfQ==";
const ROUTE: &str = "routes/index";
const NONCE: &str = "n0nce";
const BUILD: &str = "deadbeef00000001";

fn bench_pre_f03_shapes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bootstrap_pre_f03");
    group.bench_function("with_build", |b| {
        b.iter(|| hydration_bootstrap_with_build(B64, ROUTE, NONCE, BUILD));
    });
    group.bench_function("with_live/off", |b| {
        b.iter(|| hydration_bootstrap_with_live(B64, ROUTE, NONCE, BUILD, false));
    });
    group.bench_function("with_live/on", |b| {
        b.iter(|| hydration_bootstrap_with_live(B64, ROUTE, NONCE, BUILD, true));
    });
    group.finish();
}

fn bench_replay_shapes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bootstrap_replay");
    for (id, live, replay) in [
        ("off/off", false, false),
        ("off/on", false, true),
        ("on/off", true, false),
        ("on/on", true, true),
    ] {
        group.bench_with_input(
            BenchmarkId::from_parameter(id),
            &(live, replay),
            |b, (live, replay)| {
                b.iter(|| {
                    hydration_bootstrap_with_replay(B64, ROUTE, NONCE, BUILD, *live, *replay)
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_pre_f03_shapes, bench_replay_shapes);
criterion_main!(benches);
