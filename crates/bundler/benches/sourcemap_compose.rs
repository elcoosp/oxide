//! Criterion benches for the V2-C06 source-map composition seam
//! (`bench-util` gated like the A02/B09/F18/E11 benches — plain `cargo
//! test` never pays the criterion build).
//!
//! The bench-relevant questions (the composition rides every plugin
//! transform on the build path, so its cost must stay noise next to the
//! bundling itself):
//! - `sourcemap_compose/{1k,10k}`: the pure `compose` fold — the partition
//!   point per `next` token over a sorted `prev` list, the shape a plugin
//!   chain actually executes.
//! - `compose_v3_json/1k`: the full wire round trip a JS plugin map takes
//!   — parse the V3 JSON, decode the VLQ mappings, compose, re-encode,
//!   serialize.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::sourcemap_compose::{Tok, compose, compose_v3_json, v3_from_toks};

/// A deterministic token list of `n` tokens spread over `n / 10` lines.
fn token_list(n: usize) -> Vec<Tok> {
    (0..n)
        .map(|i| Tok {
            gen_line: (i / 10) as u32,
            gen_col: ((i % 10) * 7) as u32,
            src: 0,
            src_line: (i / 10) as u32,
            src_col: ((i % 10) * 5) as u32,
        })
        .collect()
}

fn bench_compose(c: &mut Criterion) {
    let mut g = c.benchmark_group("sourcemap_compose");
    for n in [1_000usize, 10_000] {
        let prev = token_list(n);
        let next = token_list(n);
        g.throughput(criterion::Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| compose(black_box(&prev), black_box(&next)));
        });
    }
    g.finish();
}

fn bench_compose_v3_json(c: &mut Criterion) {
    let mut g = c.benchmark_group("compose_v3_json");
    let prev = v3_from_toks(
        &token_list(1_000),
        Some("a.js".to_string()),
        vec!["../src/a.ts".to_string()],
        Vec::new(),
    )
    .to_json_string();
    let next = v3_from_toks(
        &token_list(1_000),
        Some("b.js".to_string()),
        vec!["a.js".to_string()],
        Vec::new(),
    )
    .to_json_string();
    g.bench_function("1k", |b| {
        b.iter(|| compose_v3_json(black_box(&prev), black_box(&next)).expect("compose"));
    });
    g.finish();
}

criterion_group!(benches, bench_compose, bench_compose_v3_json);
criterion_main!(benches);
