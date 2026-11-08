//! V2-B09: criterion benchmarks for the route matcher (`crates/router`).
//!
//! Cases (spec V2-B09): `match/{10,100,1000,5000}_routes/{static,param,catchall,miss}`
//! plus tree build time. The same bench ids run before and after each Wave B WP,
//! so `benchmarks/criterion_compare.py` can diff against `baseline-v01.json`.
//!
//! Run: `cargo bench -p oxide-fw-router --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_router::RouteTree;
use std::hint::black_box;

/// Segment alphabets for the four URL kinds (fixed depth, scale-independent).
const URLS: [(&str, &str); 4] = [
    ("static", "/s7/p7"),
    ("param", "/s0/777"),
    ("catchall", "/s7/deep/a/b/c"),
    ("miss", "/zzz/nothing/here"),
];

fn tree_for(n: usize) -> RouteTree {
    let mut t = RouteTree::new();
    for e in oxide_fw_router::testing::synthetic_entries(n) {
        t.insert(e);
    }
    // Force the derived caches (candidate index, layout chains, dispatch
    // index) so the measured loop is the steady-state hot path, not first
    // match.
    let _ = black_box(t.match_path("/s7/p7"));
    t
}

fn bench_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("match");
    for n in [10usize, 100, 1000, 5000] {
        let tree = tree_for(n);
        for (kind, url) in URLS {
            group.bench_with_input(BenchmarkId::new(n.to_string(), kind), &url, |b, url| {
                b.iter(|| black_box(tree.match_path(black_box(url))))
            });
        }
    }
    group.finish();
}

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");
    for n in [1000usize, 5000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, n| {
            b.iter(|| {
                let mut t = RouteTree::new();
                for e in oxide_fw_router::testing::synthetic_entries(*n) {
                    t.insert(e);
                }
                black_box(t.match_path("/s7/p7"));
            })
        });
    }
    group.finish();
}

/// V2-B01/B09: `RouteContextTable` build time — the boot-time cost of
/// precomputing every dispatch answer (consumed once at boot, so this is
/// reported, not gated against a per-request budget).
fn bench_context_table_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("context_table_build");
    for n in [1000usize, 5000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, n| {
            b.iter(|| {
                let entries = oxide_fw_router::testing::synthetic_entries(*n);
                black_box(oxide_fw_router::RouteContextTable::build_with_default_pattern(
                    &entries,
                ))
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_match, bench_build, bench_context_table_build);
criterion_main!(benches);
