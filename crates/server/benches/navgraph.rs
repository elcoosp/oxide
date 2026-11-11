//! Criterion benches for the V2-F13 learned-prefetch substrate
//! (`bench-util` gated like the V2-A07/V2-E04/F05/F07/F12 benches — plain
//! `cargo test` never pays the criterion build).
//!
//! The wiring-relevant questions:
//! - `nav_observe`: the per-request cost the data plane now carries — one
//!   sanitized `DashMap` upsert at 256/1024 edges. Must stay noise next to
//!   the loader run the /__data fetch triggers.
//! - `nav_top`: the `/_oxide/nav` request cost — the lazy decay + the
//!   filtered scan + the sort at 256/1024 edges out of one route.
//! - `nav_decay`: the explicit half-life pass (the flusher and every
//!   `top` call share it).
//! - `nav_persist`: the hourly snapshot write at 256 edges (the flusher's
//!   one I/O per interval).
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench navgraph`

use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::NavGraph;

/// A graph seeded with `routes` edges out of every one of 4 source routes
/// (k-anonymity lowered so the top scan ranks a full result set).
fn seeded(edges: usize) -> NavGraph {
    let g = NavGraph::with_settings(oxide_fw_server::NavGraphSettings {
        k_anon: 1.0,
        max_edges: edges * 8,
        ..oxide_fw_server::NavGraphSettings::default()
    });
    for r in 0..4usize {
        for i in 0..edges {
            g.observe(&format!("/route{r}"), &format!("/next-{i}"));
        }
    }
    g
}

fn bench_observe(c: &mut Criterion) {
    let mut group = c.benchmark_group("nav_observe");
    for edges in [256usize, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(edges), &edges, |b, e| {
            let g = seeded(*e);
            b.iter(|| {
                // One hot edge re-observed: the occupied-branch upsert the
                // data plane pays per /__data navigation.
                g.observe(black_box("/route0"), black_box("/next-0"));
            });
        });
    }
    group.finish();
}

fn bench_top(c: &mut Criterion) {
    let mut group = c.benchmark_group("nav_top");
    for edges in [256usize, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(edges), &edges, |b, e| {
            let g = seeded(*e);
            b.iter(|| {
                black_box(g.top(black_box("/route0"), black_box(3)));
            });
        });
    }
    group.finish();
}

fn bench_decay(c: &mut Criterion) {
    let mut group = c.benchmark_group("nav_decay");
    for edges in [256usize, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(edges), &edges, |b, e| {
            let g = seeded(*e);
            b.iter(|| {
                g.decay(std::time::Instant::now());
            });
        });
    }
    group.finish();
}

fn bench_persist(c: &mut Criterion) {
    let mut group = c.benchmark_group("nav_persist");
    let tmp = tempfile::tempdir().expect("tempdir");
    let path: PathBuf = tmp.path().join("bench-nav-graph.json");
    group.bench_function("256", |b| {
        let g = Arc::new(seeded(256));
        b.iter(|| {
            g.persist(black_box(&path)).expect("persist");
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_observe,
    bench_top,
    bench_decay,
    bench_persist
);
criterion_main!(benches);
