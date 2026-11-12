//! TDD pins for the hot-reload seam (the V2-F09 dev-watch follow-up:
//! the running server must pick up a rebuilt `dist/search/index`
//! without a restart).
//!
//! The contracts pinned here:
//! - [`SwappableSearchIndex`] opens a built index read-only and answers
//!   through the [`SearchIndex`] object-safe seam (the endpoint holds it
//!   exactly like the plain [`TantivyIndex`]).
//! - `try_reload` after a rebuild swaps the fresh corpus in (`Swapped`),
//!   and the swap is observable through `search`/`doc_count`.
//! - `try_reload` without a rebuild is `Unchanged` (the emitter's CAS
//!   skip leaves `meta.json` byte-identical — a reload must not thrash
//!   the mmap or the L1 cache keys).
//! - a failed reload (`SRC-101`: the directory is missing, mid-rebuild
//!   or corrupt) never degrades the running answers — the previous
//!   snapshot keeps serving (the zero-downtime posture the serve
//!   wire-up documents).
//! - `generation` bumps on a swap and only on a swap: the L1 cache
//!   partitions its keys on it, so a swapped-in corpus can never serve
//!   a pre-reload hit, while an `Unchanged` reload keeps every live
//!   cache entry valid.

use std::sync::Arc;

use oxide_fw_search::{
    ReloadOutcome, SearchDocument, SearchEngineOptions, SearchIndex, SearchQuery,
    SwappableSearchIndex, TantivyIndex,
};

/// One corpus entry: the marker lands in the body, so a query for the
/// marker distinguishes exactly which corpus is serving.
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

/// The emitter's exact rebuild sequence: the engine owns a FRESH
/// directory (a stale index is never committed onto), then commits.
fn rebuild(dir: &std::path::Path, docs: &[SearchDocument]) {
    if dir.exists() {
        std::fs::remove_dir_all(dir).unwrap();
    }
    TantivyIndex::build_in_dir(docs, &SearchEngineOptions::default(), dir)
        .expect("the rebuild commits");
}

fn search_text(idx: &dyn SearchIndex, text: &str) -> Vec<String> {
    let resp = idx
        .search(&SearchQuery::new(text))
        .expect("the query executes");
    resp.hits.iter().map(|h| h.id.clone()).collect()
}

#[test]
fn open_seeds_the_snapshot_behind_the_object_safe_seam() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(&index_dir, &[doc("posts/a", "alpha")]);

    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default())
        .expect("the built index opens");
    // The seam is object-safe and the endpoint holds it like any engine.
    let seam: Arc<dyn SearchIndex> = Arc::new(hot);
    assert_eq!(seam.doc_count(), 1);
    assert_eq!(search_text(seam.as_ref(), "alpha"), vec!["posts/a"]);
}

#[test]
fn reload_picks_up_a_rebuilt_index() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(&index_dir, &[doc("posts/a", "alpha")]);
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).unwrap();
    assert_eq!(hot.doc_count(), 1);

    // The emitter's sequence: wipe, re-commit (a new document joins).
    rebuild(
        &index_dir,
        &[doc("posts/a", "alpha"), doc("posts/b", "bravo")],
    );
    match hot.try_reload().expect("the reload opens the rebuild") {
        ReloadOutcome::Swapped { docs } => assert_eq!(docs, 2),
        ReloadOutcome::Unchanged => panic!("expected Swapped after a rebuild, got Unchanged"),
    }
    assert_eq!(hot.doc_count(), 2);
    assert_eq!(search_text(&hot, "bravo"), vec!["posts/b"]);
}

#[test]
fn reload_without_a_rebuild_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(&index_dir, &[doc("posts/a", "alpha")]);
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).unwrap();
    // No rebuild between the two reloads: the directory's meta.json is
    // byte-identical (the CAS skip's guarantee), so nothing swaps.
    assert_eq!(hot.try_reload().expect("reload"), ReloadOutcome::Unchanged);
    assert_eq!(hot.doc_count(), 1);
    assert_eq!(search_text(&hot, "alpha"), vec!["posts/a"]);
}

#[test]
fn a_failed_reload_keeps_serving_the_previous_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(
        &index_dir,
        &[doc("posts/a", "alpha"), doc("posts/b", "bravo")],
    );
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).unwrap();

    // The directory vanishes (a mid-rebuild wipe or a corrupted dist):
    // the reload refuses with the engine's open code — SRC-101 — and
    // the running answers are untouched.
    std::fs::remove_dir_all(&index_dir).unwrap();
    let err = hot.try_reload().expect_err("a missing index refuses");
    assert_eq!(err.0.code, "SRC-101");
    assert_eq!(hot.doc_count(), 2);
    assert_eq!(search_text(&hot, "bravo"), vec!["posts/b"]);
}

#[test]
fn generation_bumps_on_a_swap_and_only_on_a_swap() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(&index_dir, &[doc("posts/a", "alpha")]);
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).unwrap();
    let g0 = hot.generation();

    // An Unchanged reload keeps the generation (the live cache entries
    // stay valid — the corpus they were computed against still serves).
    assert_eq!(hot.try_reload().unwrap(), ReloadOutcome::Unchanged);
    assert_eq!(hot.generation(), g0);

    // A swap bumps it (the cache keys must partition).
    rebuild(
        &index_dir,
        &[doc("posts/a", "alpha"), doc("posts/b", "bravo")],
    );
    assert!(matches!(
        hot.try_reload().unwrap(),
        ReloadOutcome::Swapped { .. }
    ));
    assert_ne!(hot.generation(), g0);
}

#[test]
fn the_swapped_corpus_never_serves_a_stale_query_shape() {
    // The end-to-end contract the L1 cache leans on: the same query
    // text against a swapped corpus answers from the NEW corpus.
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    rebuild(&index_dir, &[doc("posts/a", "alpha")]);
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default()).unwrap();
    let _ = search_text(&hot, "bravo"); // miss on v1, would be cached by the caller

    rebuild(
        &index_dir,
        &[doc("posts/a", "alpha"), doc("posts/b", "bravo")],
    );
    assert!(matches!(
        hot.try_reload().unwrap(),
        ReloadOutcome::Swapped { .. }
    ));
    assert_eq!(search_text(&hot, "bravo"), vec!["posts/b"]);
}
