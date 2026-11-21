//! TDD pins for the rebuild wipe (the V2-F09 dev-watch follow-up's
//! emitter side: the engine owns the directory replacement, so a
//! re-emission can never lose the race against the serving reader's
//! OnCommit reloader).
//!
//! The contracts pinned here:
//! - [`wipe_dir_for_rebuild`] on a missing directory is
//!   `Ok` (the first build's shape — the caller's `create_dir_all`
//!   inside `build_in_dir` then owns creation).
//! - a populated directory is emptied: after `Ok(())` the live path is
//!   GONE (the emitter's contract — a stale index is never committed
//!   onto), not merely emptied.
//! - a crashed run's leftover `.wipe-stale` directory does not block the
//!   next wipe (the rename's target must be free).
//! - the live-watcher stress: with a serving
//!   [`SwappableSearchIndex`] open on the directory (its reloader polling
//!   every 500 ms), wipe + rebuild repeated rounds never fail. The bare
//!   `remove_dir_all` this seam replaced lost that race (~8% of rounds in
//!   the recorded loop): the reloader's tick re-created
//!   `.tantivy-meta.lock` between the wipe's child pass and its `rmdir`
//!   and the refresh died with `DirectoryNotEmpty` before the rebuild.

use oxide_fw_search::{
    SearchDocument, SearchEngineOptions, SearchIndex, SwappableSearchIndex, TantivyIndex,
    wipe_dir_for_rebuild,
};

/// One corpus entry (the same shape the hot-reload pins use).
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

#[test]
fn wipe_on_a_missing_directory_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    wipe_dir_for_rebuild(&index_dir).expect("a missing dir wipes trivially");
    assert!(!index_dir.exists(), "the missing dir stays missing");
}

#[test]
fn wipe_moves_the_live_path_out_of_the_way_entirely() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    TantivyIndex::build_in_dir(
        &[doc("posts/a", "alpha")],
        &SearchEngineOptions::default(),
        &index_dir,
    )
    .expect("the index builds");

    wipe_dir_for_rebuild(&index_dir).expect("the populated dir wipes");
    assert!(
        !index_dir.exists(),
        "the live path is GONE after the wipe — the rebuild creates it fresh, \
         a stale index is never committed onto"
    );
}

#[test]
fn a_crashed_runs_stale_leftover_does_not_block_the_next_wipe() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    let stale = dir.path().join("index.wipe-stale");
    std::fs::create_dir_all(&stale).expect("the leftover stale dir creates");
    std::fs::write(stale.join("orphan"), "a crashed run's rename target").unwrap();

    wipe_dir_for_rebuild(&index_dir).expect("the wipe clears the leftover");
    assert!(!stale.exists(), "the leftover stale dir is removed");
}

#[test]
fn wipe_plus_rebuild_survives_a_live_watched_directory() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    TantivyIndex::build_in_dir(
        &[doc("posts/a", "alpha")],
        &SearchEngineOptions::default(),
        &index_dir,
    )
    .expect("the first index builds");
    let hot = SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default())
        .expect("the serving snapshot opens");

    // The dev loop's exact repeated shape: re-emit into the directory the
    // serving reader watches, then reload. Twenty rounds at the recorded
    // ~8%/round bare-remove failure rate would hit the old race with
    // ~80% probability; the rename-side wipe makes the mechanism (a new
    // lock file at the watched path mid-wipe) structurally impossible.
    for round in 0..20 {
        let docs = if round % 2 == 0 {
            vec![doc("posts/a", "alpha"), doc("posts/b", "bravo")]
        } else {
            vec![doc("posts/a", "alpha")]
        };
        wipe_dir_for_rebuild(&index_dir).expect("the watched directory wipes for the rebuild");
        TantivyIndex::build_in_dir(&docs, &SearchEngineOptions::default(), &index_dir)
            .expect("the rebuild commits");
        let outcome = hot.try_reload().expect("the reload opens the rebuild");
        let expected = if round % 2 == 0 { 2 } else { 1 };
        match outcome {
            oxide_fw_search::ReloadOutcome::Swapped { docs } => {
                assert_eq!(docs, expected, "round {round} swapped the fresh corpus")
            }
            other => panic!("round {round}: expected Swapped, got {other:?}"),
        }
        assert_eq!(hot.doc_count(), expected, "round {round} serves fresh");
    }
}
