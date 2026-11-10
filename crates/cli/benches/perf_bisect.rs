//! Criterion benches for the V2-F18 performance-bisect substrate
//! (`bench-util` gated like the V2-A02/V2-B09/V2-F05/V2-F07 benches —
//! plain `cargo test` never pays the criterion build).
//!
//! The bench-relevant questions (the bisect runs on the build path, so the
//! pure-engine cost must stay noise next to a candidate build):
//! - `bisect/first_regressor`: the spec-verbatim search across realistic
//!   history sizes — a step measure, worst case (regression at the last
//!   probed position), O(log n) probes.
//! - `median/r{1,3,7}`: the per-probe reduction over raw samples.
//! - `rev_list_parse/1024`: the git-plumbing parser on a full first-parent
//!   history dump.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cli::perf_bisect::{bisect, median_of_runs, parse_rev_list_oldest_first};

fn history(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{i:04}")).collect()
}

fn bench_first_regressor(c: &mut Criterion) {
    let mut group = c.benchmark_group("bisect");
    for n in [16_usize, 256, 1024] {
        let commits = history(n);
        // Worst case for the search: the regression is at the end, so every
        // binary-search level is walked after the bad-end probe.
        let at = n - 1;
        group.bench_with_input(BenchmarkId::new("first_regressor", n), &at, |b, &step| {
            b.iter(|| {
                let measure = |rev: &str| {
                    if rev.parse::<usize>().unwrap_or(0) >= step {
                        300.0
                    } else {
                        100.0
                    }
                };
                bisect(black_box(&commits), measure, 100.0, 10.0)
            })
        });
    }
    group.finish();
}

fn bench_median(c: &mut Criterion) {
    let mut group = c.benchmark_group("median");
    for runs in [1_usize, 3, 7] {
        group.bench_with_input(BenchmarkId::from_parameter(runs), &runs, |b, &runs| {
            b.iter(|| {
                let mut samples: Vec<f64> = (0..runs).map(|i| 100.0 + i as f64).collect();
                median_of_runs(black_box(&mut samples))
            })
        });
    }
    group.finish();
}

fn bench_rev_list_parse(c: &mut Criterion) {
    let dump: String = (0..1024)
        .map(|i| format!("{i:040x}\n"))
        .rev()
        .collect::<Vec<_>>()
        .concat();
    c.bench_function("rev_list_parse/1024", |b| {
        b.iter(|| parse_rev_list_oldest_first(black_box(&dump)))
    });
}

criterion_group!(
    perf_bisect,
    bench_first_regressor,
    bench_median,
    bench_rev_list_parse
);
criterion_main!(perf_bisect);
