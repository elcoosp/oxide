//! Criterion benches for the §V2-A04 build + generations substrate
//! (`bench-util` gated like the A02/A05/A06/F04 benches — plain
//! `cargo test` never pays the criterion build).
//!
//! The wiring-relevant question is the per-request data-plane cost the
//! `stage_build_check` stage adds to the dispatch closure: the data-plane
//! path classification, the case-insensitive header lookup and the
//! generation acceptance — three string compares in the accepted case.

use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::build::{
    GenerationSet, advertise_build, is_data_plane_path, stage_build_check,
};
use oxide_fw_server::OxideResponse;

fn bench_data_plane_classification(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_data_plane");
    for path in ["/__data", "/_oxide/fn/thumb", "/about"] {
        group.bench_with_input(BenchmarkId::from_parameter(path), path, |b, path| {
            b.iter(|| is_data_plane_path(path));
        });
    }
    group.finish();
}

fn bench_stage_build_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("stage_build_check");
    let gens = GenerationSet::new("deadbeef0000000f".into(), Duration::from_secs(60));
    let t0 = Instant::now();
    let current = vec![("x-oxide-build".to_string(), "deadbeef0000000f".to_string())];
    let absent: Vec<(String, String)> = Vec::new();
    for (id, headers) in [("current", &current), ("absent", &absent)] {
        group.bench_with_input(BenchmarkId::from_parameter(id), headers, |b, hs| {
            b.iter(|| stage_build_check("/__data", hs, &gens, t0));
        });
    }
    group.finish();
}

fn bench_advertise_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("advertise_build");
    group.bench_function("fresh", |b| {
        b.iter(|| {
            let mut resp = OxideResponse::new(200, Vec::new());
            advertise_build(&mut resp, "deadbeef0000000f");
            resp
        });
    });
    group.bench_function("already_set", |b| {
        b.iter(|| {
            let mut resp = OxideResponse::new(200, Vec::new());
            resp.headers
                .push(("x-oxide-build".to_string(), "deadbeef0000000f".to_string()));
            advertise_build(&mut resp, "deadbeef0000000f");
            resp
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_data_plane_classification,
    bench_stage_build_check,
    bench_advertise_build
);
criterion_main!(benches);
