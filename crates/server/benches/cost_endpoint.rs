//! Criterion benches for the V2-F17 component-cost endpoint (the server
//! half), `bench-util` gated like every other criterion bench — plain
//! `cargo test` never pays the criterion build.
//!
//! The wiring-relevant question: the per-request cost the dev overlay's
//! hover pays — the param parse + the source consult + the JSON render at
//! the hit and the two refusal shapes (the missing param, the source's
//! documented 404).
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench cost_endpoint`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_server::{
    ComponentCost, CostEndpoint, CostSource, FileCostsSource, ProfiledCostSource,
    SnapshotCostSource,
};

/// A two-record source (the fixture shape — the real adapter reads the
/// bundler's module map).
struct FakeSource;

impl CostSource for FakeSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        (src == "src/hero.tsx").then(|| ComponentCost {
            bytes: 4812,
            gzip: 1901,
            chunk: "chunk-abc.js".into(),
            render_ms_p50: Some(2.5),
            cache: Some("L0".into()),
        })
    }
}

fn bench_serve(c: &mut Criterion) {
    let ep = CostEndpoint::new(Arc::new(FakeSource) as Arc<dyn CostSource>);
    let mut group = c.benchmark_group("cost_serve");
    group.bench_function("hit/full-record", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box("src=src/hero.tsx"));
            black_box(resp.status);
        });
    });
    group.bench_function("refused/missing-param", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box(""));
            black_box(resp.status);
        });
    });
    group.bench_function("refused/unknown-src", |b| {
        b.iter(|| {
            let resp = ep.serve(black_box("GET"), black_box("src=src/ghost.tsx"));
            black_box(resp.status);
        });
    });
}

fn bench_snapshot_source(c: &mut Criterion) {
    // The adapter the dev boot feeds: the bundler's snapshot records in
    // memory — the lookup the endpoint pays per query, at the realistic
    // (one module per source) and scaled site shapes.
    let mut group = c.benchmark_group("cost_source_snapshot");
    for n in [64usize, 4096] {
        let records: std::collections::BTreeMap<String, ComponentCost> = (0..n)
            .map(|i| {
                (
                    format!("src/c{i}.tsx"),
                    ComponentCost::bytes_only(
                        1024 + u64::try_from(i).unwrap_or(0),
                        400,
                        "chunk-abc.js",
                    ),
                )
            })
            .collect();
        let source = SnapshotCostSource::new(records);
        group.bench_with_input(BenchmarkId::from_parameter(n), &source, |b, s| {
            b.iter(|| {
                black_box(s.cost(black_box("src/c7.tsx")));
                black_box(s.cost(black_box("src/ghost.tsx")));
            });
        });
    }
    group.finish();
}

fn bench_profiled_source(c: &mut Criterion) {
    // The profiler + tier-map adapter (the record's remaining halves):
    // the decorated hit pays the base lookup plus the two map probes —
    // the per-query cost over the same realistic and scaled shapes, the
    // same hit/miss pair the snapshot group measures (the decorator's
    // overhead is the delta).
    let mut group = c.benchmark_group("cost_source_profiled");
    for n in [64usize, 4096] {
        let records: std::collections::BTreeMap<String, ComponentCost> = (0..n)
            .map(|i| {
                (
                    format!("src/c{i}.tsx"),
                    ComponentCost::bytes_only(
                        1024 + u64::try_from(i).unwrap_or(0),
                        400,
                        "chunk-abc.js",
                    ),
                )
            })
            .collect();
        let source = ProfiledCostSource::from_maps(
            Arc::new(SnapshotCostSource::new(records)) as Arc<dyn CostSource>,
            (0..n)
                .filter(|i| i % 4 == 0)
                .map(|i| (format!("src/c{i}.tsx"), 1.5)),
            (0..n)
                .filter(|i| i % 2 == 0)
                .map(|i| (format!("src/c{i}.tsx"), "L0".to_string())),
        );
        group.bench_with_input(BenchmarkId::from_parameter(n), &source, |b, s| {
            b.iter(|| {
                black_box(s.cost(black_box("src/c7.tsx")));
                black_box(s.cost(black_box("src/ghost.tsx")));
            });
        });
    }
    group.finish();
}

fn bench_file_source(c: &mut Criterion) {
    // The dev-side feed the boot attaches: the build's PERSISTED records
    // file, mtime-checked per query — the stat the dev overlay's hover
    // pays on every consult, plus the cached lookup (the reload only
    // fires when the file's mtime moved, so the steady-state hit is the
    // per-request shape). Scaled like the snapshot group: the realistic
    // small site and a large module map.
    let mut group = c.benchmark_group("cost_source_file");
    for n in [64usize, 4096] {
        let dir = tempfile::tempdir().expect("bench tempdir");
        let path = dir.path().join("client.costs.json");
        let rows: Vec<serde_json::Value> = (0..n)
            .map(|i| {
                serde_json::json!({
                    "source": format!("src/c{i}.tsx"),
                    "bytes": 1024_u64 + u64::try_from(i).unwrap_or(0),
                    "gzip": 400_u64,
                    "chunk": "chunk-abc.js",
                })
            })
            .collect();
        std::fs::write(&path, serde_json::to_string(&rows).expect("rows")).expect("records write");
        let source = FileCostsSource::from_path(&path);
        group.bench_with_input(BenchmarkId::from_parameter(n), &source, |b, s| {
            b.iter(|| {
                black_box(s.cost(black_box("src/c7.tsx")));
                black_box(s.cost(black_box("src/ghost.tsx")));
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_serve,
    bench_snapshot_source,
    bench_profiled_source,
    bench_file_source
);
criterion_main!(benches);
