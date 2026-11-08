//! Criterion benches for the §15.4 experiments assignment engine
//! (V2-F04, `bench-util` gated like the V2-A02/V2-A05/V2-A06 benches —
//! plain `cargo test` never pays the criterion build).
//!
//! The wiring-relevant question is the per-request cost of the native
//! evaluation the `stage_experiments` closure adds: `assign` per
//! experiment (the blake3 bucket) and `resolve` across a route-scaled
//! experiment list with all three sticky strategies in play.

use std::cell::Cell;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::experiments::{Experiment, Sticky, assign, resolve};

/// One experiment exercising the given sticky strategy with `variants`
/// variants and the given route prefixes (empty = all routes).
fn experiment(id: &str, sticky: Sticky, variants: usize, routes: &[&str]) -> Experiment {
    Experiment {
        id: id.to_string(),
        traffic: 100,
        sticky,
        variants: (0..variants).map(|i| format!("variant-{i}")).collect(),
        routes: routes.iter().map(|r| (*r).to_string()).collect(),
    }
}

fn bench_assign(c: &mut Criterion) {
    let mut group = c.benchmark_group("assign");
    for variants in [2usize, 8, 16] {
        let e = experiment("hero", Sticky::IpHash, variants, &[]);
        group.bench_with_input(BenchmarkId::from_parameter(variants), &variants, |b, _| {
            b.iter(|| {
                for i in 0..100u64 {
                    black_box(assign(black_box(&e), black_box(&format!("identity-{i}"))));
                }
            });
        });
    }
    group.finish();
}

fn bench_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve");
    for count in [1usize, 8] {
        // All three sticky strategies across the list; cookie identity
        // present so no minting happens on the hot path.
        let exps: Vec<Experiment> = (0..count)
            .flat_map(|i| {
                let n = i.to_string();
                [
                    experiment(
                        &format!("cookie-{n}"),
                        Sticky::Cookie(format!("oxide-{n}")),
                        4,
                        &[],
                    ),
                    experiment(
                        &format!("header-{n}"),
                        Sticky::Header("x-bucket".into()),
                        4,
                        &[],
                    ),
                    experiment(&format!("ip-{n}"), Sticky::IpHash, 4, &[]),
                ]
            })
            .collect();
        let cookie = "oxide-0=b0; oxide-1=b1";
        let header = |name: &str| (name == "x-bucket").then(|| "cohort".to_string());
        let mints = Cell::new(0u32);
        let mint = || {
            let n = mints.get() + 1;
            mints.set(n);
            format!("minted-{n}")
        };
        group.bench_with_input(BenchmarkId::from_parameter(count * 3), &count, |b, _| {
            b.iter(|| {
                black_box(resolve(
                    black_box(&exps),
                    black_box("/pricing/enterprise"),
                    black_box(Some(cookie)),
                    &header,
                    black_box("203.0.113.7"),
                    &mint,
                ));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_assign, bench_resolve);
criterion_main!(benches);
