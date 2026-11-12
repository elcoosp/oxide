//! The hot-reload walkthrough (the V2-F09 dev-watch follow-up): build a
//! corpus into a directory, open the swappable serve-side handle, query,
//! rebuild the directory the way the CLI's emitter does (fresh
//! directory — a stale index is never committed onto), reload without a
//! restart and query again. The `generation` the endpoint's L1 cache
//! partitions on bumps exactly once, on the swap.
//!
//! Run with: `cargo run -p oxide-fw-search --example hot_reload`
//!
//! No filesystem fixture is required: the corpus is inline, and the
//! directory lives in a tempdir the example cleans up.

use std::path::Path;

use oxide_fw_search::{
    ReloadOutcome, SearchDocument, SearchEngineOptions, SearchIndex, SearchQuery,
    SwappableSearchIndex, TantivyIndex,
};

/// One corpus entry: the marker lands in the body, so a query for it
/// tells which corpus is serving.
fn doc(id: &str, marker: &str) -> SearchDocument {
    SearchDocument {
        id: id.to_string(),
        url: format!("/{id}"),
        title: format!("the {id} guide"),
        body: format!("{marker} is the body marker of {id}."),
        tags: vec!["guide".into()],
        lang: "en".into(),
    }
}

/// The emitter's rebuild sequence: the engine owns a FRESH directory.
fn rebuild(dir: &Path, docs: &[SearchDocument]) -> Result<(), Box<dyn std::error::Error>> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    TantivyIndex::build_in_dir(docs, &SearchEngineOptions::default(), dir)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber_for_example();
    let tmp = tempfile::tempdir()?;
    let index_dir = tmp.path().join("index");

    // 1. The build side emits v1; the serve side opens it read-only.
    rebuild(&index_dir, &[doc("posts/a", "alpha")])?;
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default())?;
    println!(
        "opened: {} docs, generation {}",
        hot.doc_count(),
        hot.generation()
    );
    println!("`alpha` hits: {:?}", ids(&hot, "alpha"));

    // 2. A reload with no rebuild underneath is Unchanged — the CAS
    //    skip's guarantee — and the L1's live entries stay valid.
    println!("reload (no rebuild): {:?}", hot.try_reload()?);
    println!("generation: {}", hot.generation());

    // 3. The dev loop's content change: the emitter rebuilds (a second
    //    document joins), the serve side swaps it in without a restart.
    rebuild(
        &index_dir,
        &[doc("posts/a", "alpha"), doc("posts/b", "bravo")],
    )?;
    match hot.try_reload()? {
        ReloadOutcome::Swapped { docs } => println!("reload: Swapped {{ docs: {docs} }}"),
        ReloadOutcome::Unchanged => println!("reload: Unchanged"),
    }
    println!("generation: {}", hot.generation());
    println!("`bravo` hits: {:?}", ids(&hot, "bravo"));

    // 4. The directory vanishing (a mid-rebuild wipe) refuses with the
    //    engine's open code — and the previous snapshot keeps serving.
    std::fs::remove_dir_all(&index_dir)?;
    match hot.try_reload() {
        Err(e) => println!("reload refused: {} (the snapshot keeps serving)", e.0.code),
        Ok(other) => println!("unexpected: {other:?}"),
    }
    println!("`alpha` still: {:?}", ids(&hot, "alpha"));
    Ok(())
}

/// Query and print the hit identities.
fn ids(hot: &SwappableSearchIndex, text: &str) -> Vec<String> {
    hot.search(&SearchQuery::new(text))
        .expect("the query executes")
        .hits
        .iter()
        .map(|h| h.id.clone())
        .collect()
}

/// A minimal tracing subscriber so the engine's structured events are
/// visible (the workspace's observability convention).
fn tracing_subscriber_for_example() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();
}
