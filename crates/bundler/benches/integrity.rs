//! Criterion benches for the integrity verifier (the V2-E04 mismatch-case
//! tail, `bench-util` gated — plain `cargo test` never pays the criterion
//! build).
//!
//! The bench-relevant question: `oxide verify` walks the whole `integrity`
//! map on every run — a read + SHA-384 + base64 per recorded file — so its
//! cost must stay proportional to the dist, not the codebase. The groups:
//! - `integrity_verify/10`: a small dist (the smoke-app shape).
//! - `integrity_verify/100`: a large dist (a full site's chunk count).

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::sri::{sri_digest, verify_integrity_map};

/// A deterministic chunk-shaped body (the sri bench's shape, not a single
/// repeated byte, which some hash trees fold cheaply).
fn chunk_shaped_body(size: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(size);
    while body.len() < size {
        body.extend_from_slice(
            format!("export const s{} = \"chunk-{}\";\n", body.len(), body.len()).as_bytes(),
        );
    }
    body.truncate(size);
    body
}

/// Write `count` emitted files under `dir` and record their digests — the
/// `dist/client/` + manifest `integrity` shape at verify time.
fn emitted_fixture(dir: &Path, count: usize) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for i in 0..count {
        let name = format!("index-{i}.js");
        let body = chunk_shaped_body(16 * 1024);
        std::fs::write(dir.join(&name), &body).expect("write emitted file");
        map.insert(name, sri_digest(&body));
    }
    map
}

fn bench_verify_integrity_map(c: &mut Criterion) {
    let mut g = c.benchmark_group("integrity_verify");
    for count in [10, 100] {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = emitted_fixture(dir.path(), count);
        g.throughput(criterion::Throughput::Elements(count as u64));
        g.bench_with_input(BenchmarkId::from_parameter(count), &map, |b, map| {
            b.iter(|| verify_integrity_map(black_box(Path::new(dir.path())), black_box(map)));
        });
    }
    g.finish();
}

criterion_group!(benches, bench_verify_integrity_map);
criterion_main!(benches);
