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

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_server::{ComponentCost, CostEndpoint, CostSource};

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

criterion_group!(benches, bench_serve);
criterion_main!(benches);
