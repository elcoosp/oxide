//! Criterion benches for the search endpoint's request gate (`search` +
//! `bench-util` gated — plain `cargo test` never pays the criterion build).
//!
//! The reattach-relevant questions:
//! - `over_cap_refusal`: the §13.4 query-length gate's cost on the refusal
//!   path — the 400 an oversized query gets before the L1 or the engine.
//! - `l1_hit`: an under-cap query served from the L1 (60 s) — the hot
//!   repeat-search path the cache exists for.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_search::{SearchDocument, SearchEngineOptions, TantivyIndex};
use oxide_fw_server::search_endpoint::SearchEndpoint;

/// The TDD fixture's engine shape (three documents, two languages).
fn engine() -> Arc<SearchEndpoint> {
    let docs = vec![
        SearchDocument {
            id: "posts/a".into(),
            url: "/posts/a".into(),
            title: "the rust guide".into(),
            body: "rust is a systems language. it ships an indexer.".into(),
            tags: vec!["guide".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/b".into(),
            url: "/posts/b".into(),
            title: "shipping notes".into(),
            body: "the word rust appears in the body here.".into(),
            tags: vec!["api".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/c".into(),
            url: "/posts/c".into(),
            title: "der deutsche artikel".into(),
            body: "dieser text spricht ueber rust im allgemeinen.".into(),
            tags: vec!["guide".into()],
            lang: "de".into(),
        },
    ];
    let idx = TantivyIndex::build_in_ram(&docs, &SearchEngineOptions::default()).expect("build");
    Arc::new(SearchEndpoint::new(Arc::new(idx)))
}

fn bench_search_gate(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_gate");
    let ep = engine();

    // Warm the L1 once: the `l1_hit` shape measures the cached path.
    let warm = ep.run(&oxide_fw_search::SearchQuery {
        text: "rust".into(),
        limit: 10,
        tag: None,
        lang: None,
    });
    let _ = warm.expect("the warm query indexes");

    group.bench_function("over_cap_refusal", |b| {
        let oversized = "q=".to_string() + &"r".repeat(5000);
        b.iter(|| {
            black_box(oxide_fw_server::search_endpoint::search_response(
                black_box(&ep),
                black_box(&oversized),
            ))
        });
    });
    group.bench_function("l1_hit", |b| {
        b.iter(|| {
            black_box(oxide_fw_server::search_endpoint::search_response(
                black_box(&ep),
                black_box("q=rust"),
            ))
        });
    });
    group.finish();
}

criterion_group!(benches, bench_search_gate);
criterion_main!(benches);
