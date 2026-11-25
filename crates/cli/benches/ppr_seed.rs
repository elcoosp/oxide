//! §10.4 / P0-4 bench: the build-time PPR shell seed's per-page cost. The
//! split rides the prerender walk (one extra pass over each rendered page
//! when `render.ppr` is on — the page render itself dwarfs it), the
//! manifest encode/parse ride the build write and the boot seed. The
//! boundary-free group is the honest zero-work shape (the collector's
//! skip). Runs in test mode (`cargo test -p oxide-fw-cli --features
//! bench-util --benches -- --test` or `cargo bench -p oxide-fw-cli
//! --features bench-util`).
//!
//! The bench is gated behind `bench-util` (the V2-F18 pattern: plain
//! `cargo test` never pays the criterion build).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::{
    PprSeedSource, parse_ppr_shell_seed, ppr_shell_seed_manifest, ppr_shell_seed_rows,
};
use std::hint::black_box as bb;

/// One rendered page: a head with the sentineled nonce, a hero, N boundary
/// templates (the fallback inside), the data-ox-c fragments the streaming
/// render appends.
fn fixture_page(boundaries: usize) -> String {
    let mut html = String::from(
        "<!doctype html><html><head><meta nonce=\"__OXIDE_L0_NONCE__\">\
<style>body{margin:0}</style></head><body><header>site</header><main>",
    );
    for i in 0..boundaries {
        html.push_str(&format!(
            "<section><template data-ox-b='b{i}'><p>fallback {i}</p></template>\
<template data-ox-c='b{i}'><p>resolved {i}</p></template></section>"
        ));
    }
    html.push_str("</main><footer>site</footer></body></html>");
    html
}

fn source_rows(pages: usize, boundaries: usize) -> Vec<PprSeedSource> {
    (0..pages)
        .map(|p| PprSeedSource {
            route_id: format!("routes/page{p}"),
            url_path: format!("/page{p}"),
            params: String::new(),
            sentineled_html: fixture_page(boundaries),
            locale: String::new(),
        })
        .collect()
}

fn bench_seed_split(c: &mut Criterion) {
    let mut group = c.benchmark_group("ppr_seed");
    let vary = "accept-language=";
    group.bench_function("split/one-boundary", |b| {
        let sources = source_rows(1, 1);
        b.iter(|| ppr_shell_seed_rows(bb(&sources), "b1", bb(vary), "en"))
    });
    group.bench_function("split/eight-boundaries", |b| {
        let sources = source_rows(1, 8);
        b.iter(|| ppr_shell_seed_rows(bb(&sources), "b1", bb(vary), "en"))
    });
    group.bench_function("split/boundary-free-skip", |b| {
        let sources = source_rows(1, 0);
        b.iter(|| ppr_shell_seed_rows(bb(&sources), "b1", bb(vary), "en"))
    });
    let rows = ppr_shell_seed_rows(&source_rows(64, 4), "b1", vary, "en");
    group.bench_function("manifest/encode-64-rows", |b| {
        b.iter(|| ppr_shell_seed_manifest(bb(&rows), "b1"))
    });
    let bytes = ppr_shell_seed_manifest(&rows, "b1");
    group.bench_function("manifest/boot-parse-64-rows", |b| {
        b.iter(|| parse_ppr_shell_seed(bb(&bytes), "b1"))
    });
    group.finish();
}

criterion_group!(benches, bench_seed_split);
criterion_main!(benches);
