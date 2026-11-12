//! Criterion benches for the V2-F09 search engine (`bench-util` gated
//! like every other criterion bench in the workspace — plain `cargo
//! test` never pays the criterion build).
//!
//! The acceptance-relevant questions (§V2-F09: "a 5,000-document corpus
//! builds in under 5 s and answers p99 under 5 ms"):
//! - `search_build`: the index build at 100/5,000 documents (the build
//!   side's one-time cost per configured collection).
//! - `search_query`: the per-request query cost on the 5,000-document
//!   corpus (the serve side's per-request budget; the L1 cache sits on
//!   top of this, so an uncached query is the worst case).
//! - `search_static_export`: the `static.json` build at 1,000 documents
//!   (the build side's second output).
//! - `search_hot_reload`: the dev loop's per-content-change reload price
//!   — the unchanged fast path (one `meta.json` read + byte compare) and
//!   the full rebuild-then-swap sequence (the rebuild cost itself is the
//!   `search_build` group's question).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_search::{
    SearchDocument, SearchEngineOptions, SearchIndex, SearchQuery, SwappableSearchIndex,
    TantivyIndex,
};

/// A synthetic corpus: `docs` entries with a private title term, a
/// private body term and shared filler (realistic term-frequency shape,
/// unambiguous winners for every doc).
fn corpus(docs: usize) -> Vec<SearchDocument> {
    (0..docs)
        .map(|i| SearchDocument {
            id: format!("posts/{i}"),
            url: format!("/posts/{i}"),
            title: format!("alpha{i} field notes"),
            body: format!(
                "omega{i} entry about the framework and its pipelines with shared filler words here"
            ),
            tags: vec![format!("t{}", i % 16)],
            lang: "en".into(),
        })
        .collect()
}

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_build");
    for docs in [100usize, 5_000] {
        let docs_vec = corpus(docs);
        group.bench_with_input(BenchmarkId::from_parameter(docs), &docs_vec, |b, d| {
            b.iter(|| {
                TantivyIndex::build_in_ram(black_box(d), &SearchEngineOptions::default())
                    .expect("build")
            });
        });
    }
    group.finish();
}

fn bench_query(c: &mut Criterion) {
    let docs_vec = corpus(5_000);
    let idx =
        TantivyIndex::build_in_ram(&docs_vec, &SearchEngineOptions::default()).expect("build");
    let mut group = c.benchmark_group("search_query");
    // Criterion writes one directory per parameter — the label must be
    // path-safe (the queries themselves carry spaces).
    for (label, q) in [
        ("single-title-term", "alpha4000"),
        ("single-body-term", "entry"),
        ("two-terms", "alpha4000 omega4000"),
        ("phrase", "the framework pipelines"),
    ] {
        group.bench_with_input(BenchmarkId::from_parameter(label), &q, |b, q| {
            b.iter(|| {
                idx.search(black_box(&SearchQuery {
                    text: (*q).to_string(),
                    ..SearchQuery::default()
                }))
                .expect("query")
            });
        });
    }
    group.finish();
}

fn bench_static_export(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_static_export");
    let docs_vec = corpus(1_000);
    group.bench_with_input(
        BenchmarkId::from_parameter(1_000usize),
        &docs_vec,
        |b, d| {
            b.iter(|| {
                let st = oxide_fw_search::StaticIndex::build(black_box(d));
                st.to_json().expect("json")
            });
        },
    );
    group.finish();
}

fn bench_hot_reload(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let index_dir = dir.path().join("index");
    let docs_vec = corpus(100);
    TantivyIndex::build_in_dir(&docs_vec, &SearchEngineOptions::default(), &index_dir)
        .expect("build");
    let hot =
        SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).expect("open");
    let mut group = c.benchmark_group("search_hot_reload");
    // The CAS-skip fast path: the directory is untouched, the reload is
    // one small file read and a byte compare.
    group.bench_function("unchanged_100", |b| {
        b.iter(|| hot.try_reload().expect("reload"));
    });
    // The full dev-loop sequence per content change: wipe, re-commit,
    // reload (the swap opens the rebuild and swaps it in).
    group.bench_function("rebuild_100_then_reload", |b| {
        b.iter(|| {
            if index_dir.exists() {
                std::fs::remove_dir_all(&index_dir).expect("wipe");
            }
            TantivyIndex::build_in_dir(
                black_box(&docs_vec),
                &SearchEngineOptions::default(),
                &index_dir,
            )
            .expect("build");
            hot.try_reload().expect("reload")
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_build,
    bench_query,
    bench_static_export,
    bench_hot_reload
);
criterion_main!(benches);
