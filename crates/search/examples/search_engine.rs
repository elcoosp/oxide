//! The V2-F09 engine walkthrough: build a small corpus into RAM and into
//! a directory, query it (ranking, fuzzy, filters, snippets), export the
//! static fallback and read it back.
//!
//! Run with: `cargo run -p oxide-fw-search --example search_engine`
//!
//! No filesystem fixture is required: the corpus is inline, and the
//! directory-backed build lands in a tempdir the example cleans up.

use std::path::PathBuf;

use oxide_fw_config::SearchConfig;
use oxide_fw_search::{
    SearchDocument, SearchEngineOptions, SearchIndex, SearchQuery, StaticIndex, TantivyIndex,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber_for_example();
    let docs = vec![
        SearchDocument {
            id: "posts/a".into(),
            url: "/posts/a".into(),
            title: "the rust guide".into(),
            body: "rust is a systems language. it ships an indexer and a query engine.".into(),
            tags: vec!["guide".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/b".into(),
            url: "/posts/b".into(),
            title: "shipping notes".into(),
            body: "the word rust appears in the body here, not the title.".into(),
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

    // The typed config threads its knobs into the engine (title 3, body
    // 1, snippet 160 by default — the spec's documented values).
    let cfg = SearchConfig::default();
    let opts = SearchEngineOptions::from_config(&cfg);
    println!("options: {opts:?}");
    println!(
        "content hash (CAS key): {}",
        TantivyIndex::content_hash(&docs)
    );

    // 1. the Full engine, in RAM.
    let idx = TantivyIndex::build_in_ram(&docs, &opts)?;
    println!("doc count: {}", idx.doc_count());

    // 2. the ranking pin: a title match outranks a body match.
    let resp = idx.search(&SearchQuery::new("rust"))?;
    println!("\nquery `rust` (total {}):", resp.total);
    for hit in &resp.hits {
        println!(
            "  {} {} score {:.2} — {}",
            hit.id, hit.url, hit.score, hit.snippet
        );
    }

    // 3. the fuzzy pin: `kernl` reaches `kernel` at distance 1.
    let resp = idx.search(&SearchQuery::new("kernl"))?;
    println!("\nquery `kernl` (total {}):", resp.total);

    // 4. the filter pin: only the guide-tagged docs.
    let resp = idx.search(&SearchQuery {
        tag: Some("guide".into()),
        ..SearchQuery::new("rust")
    })?;
    println!("query `rust` tag=guide (total {}):", resp.total);

    // 5. the dir-backed build + the serve side's read-only open.
    let dir = tempfile::Builder::new()
        .prefix("oxide-search-example")
        .tempdir()?;
    TantivyIndex::build_in_dir(&docs, &opts, dir.path())?;
    drop(idx);
    let reopened = TantivyIndex::open_read_only(dir.path())?.with_options(&opts);
    println!("\nreopened read-only: {} docs", reopened.doc_count());

    // 6. the static fallback: the export shape and the degraded matcher.
    let st = StaticIndex::build(&docs);
    let json = st.to_json()?;
    println!("\nstatic.json ({} bytes)", json.len());
    let matcher = oxide_fw_search::StaticMatcher::new(
        StaticIndex::from_json(&json)?,
        SearchEngineOptions::default(),
    );
    let resp = matcher.search(&SearchQuery::new("rust"))?;
    println!("matcher `rust` (total {}):", resp.total);
    for hit in &resp.hits {
        println!("  {} score {:.2}", hit.id, hit.score);
    }

    println!(
        "\nendpoint: GET /_oxide/search?q=<text> ({})",
        oxide_fw_search::SEARCH_ENDPOINT_PATH
    );
    let _ = PathBuf::new(); // keep the import honest for hosts extending the walkthrough
    Ok(())
}

/// A minimal tracing subscriber so the engine's structured events are
/// visible (the workspace's observability convention).
fn tracing_subscriber_for_example() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();
}
