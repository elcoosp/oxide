//! Criterion benches for the V2-E04 SRI digest pass (`bench-util` gated
//! like the A02/B09/F18/E11 benches — plain `cargo test` never pays the
//! criterion build).
//!
//! The bench-relevant questions (the digests ride every client build, so
//! their cost must stay noise next to the bundling itself):
//! - `sri_digest/{4kb,64kb}`: the SHA-384 + standard-base64 pass over a
//!   chunk-sized and an asset-sized body — the chunk bundler digests bytes
//!   already in hand, so this is the whole marginal cost per emitted file.
//! - `integrity_map/10`: the rolldown path's read-back walk over a
//!   ten-file emitted set (rolldown persisted the files itself, so the
//!   pass reads them back from `out_dir` — the same bytes the browser
//!   fetches, hashed exactly once).

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::sri::{integrity_map, sri_digest};

fn chunk_shaped_body(size: usize) -> Vec<u8> {
    // A deterministic, realistic-looking JS body (repeated small statements
    // — not a single repeated byte, which some hash trees fold cheaply).
    let mut body = Vec::with_capacity(size);
    while body.len() < size {
        body.extend_from_slice(
            format!("export const s{} = \"chunk-{}\";\n", body.len(), body.len()).as_bytes(),
        );
    }
    body.truncate(size);
    body
}

fn bench_sri_digest(c: &mut Criterion) {
    let mut g = c.benchmark_group("sri_digest");
    for size in [4 * 1024, 64 * 1024] {
        let body = chunk_shaped_body(size);
        g.throughput(criterion::Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &body, |b, body| {
            b.iter(|| sri_digest(black_box(body)));
        });
    }
    g.finish();
}

fn bench_integrity_map(c: &mut Criterion) {
    let mut g = c.benchmark_group("integrity_map");
    let dir = tempfile::tempdir().expect("tempdir");
    let files: Vec<String> = (0..10)
        .map(|i| {
            let name = format!("index-{i}.js");
            std::fs::write(dir.path().join(&name), chunk_shaped_body(16 * 1024))
                .expect("write emitted file");
            name
        })
        .collect();
    g.throughput(criterion::Throughput::Elements(files.len() as u64));
    g.bench_function("10", |b| {
        b.iter(|| integrity_map(black_box(Path::new(dir.path())), black_box(&files)));
    });
    g.finish();
}

criterion_group!(benches, bench_sri_digest, bench_integrity_map);
criterion_main!(benches);
