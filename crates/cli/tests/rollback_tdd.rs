//! TDD pins for the §V2-F10 step-4 rollback slice — the retained
//! generations layout (`.oxide/generations/<id>/dist`), the resolution
//! seam (`previous` / explicit id) and the dist repoint swap
//! (`oxide_fw_cli::serve::generations`, spec v0.2 §V2-F10).
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before the retention core existed — the same split every `*_tdd.rs`
//! suite in this crate uses: the pure engine is pinned, the process shell
//! (the `oxide rollback` command, the serve boot's `--generation` leg and
//! the SIGHUP signal) is wired and reviewed.
//!
//! Spec anchors (docs/spec-v0.2-draft.md §V2-F10 step 4 and its Tests
//! list):
//! - "Keep `.oxide/generations/<id>` as hard-linked copies (or symlinks
//!   on Windows fallback to copy) of the last N=3 builds, written at the
//!   end of `phase_emit`."
//! - "`oxide start --generation previous` or `oxide rollback` repoints
//!   `dist` and sends `SIGHUP`. No rebuild."
//! - Tests: "`rollback_swaps_dist` against a temp dir."
//!
//! The deviations record (docs/deviations.md, "V2-F10 skew protection —
//! the two-pool serving deltas", delta 3) is the milestone's contract:
//! "The rollback path's `--generation previous` flag and the
//! `.oxide/generations/<id>` hard-link layout (spec step 4) stay deferred
//! — the snapshot directory now doubles as the frozen-source store; the
//! flag rides the F11 releases slice."

use std::path::Path;

use oxide_fw_cli::serve::generations::{
    repoint_dist_to_generation, resolve_generation, retain_dist_generation, retained_generations,
};

/// Write a fake but honestly-stamped dist (the A04 contract: the id is a
/// digest of the covered bytes, so different content mint different ids
/// through the same [`oxide_fw_cache::stamp_build_id`] the real build
/// calls at the end of `phase_emit`).
fn stamp_dist(root: &Path, body: &[u8]) -> String {
    let dist = root.join("dist");
    std::fs::create_dir_all(dist.join("client/assets")).expect("mkdir");
    std::fs::write(dist.join("client/assets/a.js"), body).expect("write");
    std::fs::write(dist.join("client/index.html"), b"<html></html>").expect("write");
    oxide_fw_cache::stamp_build_id(&dist).expect("stamp")
}

#[test]
fn rollback_swaps_dist() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();

    // Build v1, retain it; rebuild v2, retain it too. The retention runs
    // at the end of `phase_emit` in the real pipeline — here the same
    // seam is driven directly.
    let v1 = stamp_dist(root, b"v1");
    retain_dist_generation(root, &v1, 3).expect("retain v1");
    let v2 = stamp_dist(root, b"v2");
    retain_dist_generation(root, &v2, 3).expect("retain v2");

    // The current dist IS v2; `previous` resolves v1's retained copy.
    let gen_dist = resolve_generation(root, "previous", Some(&v2)).expect("resolve previous");
    assert_eq!(
        oxide_fw_cache::read_build_id(&gen_dist).expect("read"),
        v1,
        "previous names the retained v1, not the current v2"
    );

    // The swap: dist now serves v1, byte for byte (the spec's pin).
    repoint_dist_to_generation(root, &gen_dist).expect("repoint");
    let dist = root.join("dist");
    assert_eq!(
        oxide_fw_cache::read_build_id(&dist).expect("read"),
        v1,
        "dist carries the rolled-back id"
    );
    assert_eq!(
        std::fs::read(dist.join("client/assets/a.js")).expect("read bytes"),
        b"v1",
        "dist carries the rolled-back bytes"
    );
}

#[test]
fn retention_keeps_the_last_n_builds() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();

    let v1 = stamp_dist(root, b"v1");
    retain_dist_generation(root, &v1, 2).expect("retain v1");
    let v2 = stamp_dist(root, b"v2");
    retain_dist_generation(root, &v2, 2).expect("retain v2");
    let v3 = stamp_dist(root, b"v3");
    retain_dist_generation(root, &v3, 2).expect("retain v3");

    let kept = retained_generations(root);
    assert_eq!(kept.len(), 2, "the keep bound holds");
    assert_eq!(kept[0], v3, "newest first");
    assert_eq!(kept[1], v2, "the oldest build is pruned past the bound");
    assert!(
        !root
            .join(".oxide/generations")
            .join(&v1)
            .join("dist")
            .exists(),
        "v1's retained dist tree is gone"
    );
}

#[test]
fn retaining_the_same_build_id_again_is_a_no_op() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    let id = stamp_dist(root, b"only");
    let first = retain_dist_generation(root, &id, 3).expect("retain");
    let second = retain_dist_generation(root, &id, 3).expect("retain again");
    assert_eq!(first, second, "the same id resolves to the same copy");
    assert_eq!(retained_generations(root).len(), 1, "one entry, not two");
}

#[test]
fn resolve_refuses_an_unknown_generation_and_no_previous() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();

    // Nothing retained at all: `previous` has nothing to name.
    let err = resolve_generation(root, "previous", None).expect_err("no generations yet");
    assert!(
        format!("{err}").contains("SKW-GEN"),
        "the refusal is coded SKW-GEN, got: {err}"
    );

    // An explicit id that was never retained refuses the same way.
    let err = resolve_generation(root, "0123456789abcdef", None).expect_err("unknown id");
    assert!(
        format!("{err}").contains("SKW-GEN"),
        "coded refusal, got: {err}"
    );

    // A retained generation resolves by explicit id — including with the
    // id that is current (an explicit request never compares against the
    // current id; only `previous` does).
    let id = stamp_dist(root, b"kept");
    retain_dist_generation(root, &id, 3).expect("retain");
    let gen_dist = resolve_generation(root, &id, Some(&id)).expect("explicit id");
    assert_eq!(oxide_fw_cache::read_build_id(&gen_dist).expect("read"), id);
}

#[test]
fn resolve_refuses_a_spec_that_is_not_a_generation_name() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    // A path-shaped spec is a traversal attempt, not a generation name —
    // the trust boundary is the generations directory itself.
    for bad in ["../escape", "a/b", "", ".", ".."] {
        let err = resolve_generation(root, bad, None).expect_err("path-shaped spec");
        assert!(
            format!("{err}").contains("SKW-GEN"),
            "`{bad}` refuses coded, got: {err}"
        );
    }
}

#[test]
fn repointing_to_the_current_build_is_a_no_op() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    let id = stamp_dist(root, b"current");
    retain_dist_generation(root, &id, 3).expect("retain");
    let gen_dist = resolve_generation(root, &id, Some(&id)).expect("resolve");
    // The live dist already serves this id: the swap must not churn the
    // tree (an idempotent re-deploy never disturbs the served bytes).
    repoint_dist_to_generation(root, &gen_dist).expect("repoint");
    assert!(root.join("dist/client/assets/a.js").exists(), "dist intact");
}

#[test]
fn resolve_previous_skips_the_current_id() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    let v1 = stamp_dist(root, b"v1");
    retain_dist_generation(root, &v1, 3).expect("retain v1");
    let v2 = stamp_dist(root, b"v2");
    retain_dist_generation(root, &v2, 3).expect("retain v2");

    // Rolling back twice alternates: with v2 current, `previous` is v1;
    // with v1 current (after the swap), `previous` is v2 — the retained
    // copy the swap left behind.
    let to_v1 = resolve_generation(root, "previous", Some(&v2)).expect("resolve");
    assert_eq!(oxide_fw_cache::read_build_id(&to_v1).expect("read"), v1);
    repoint_dist_to_generation(root, &to_v1).expect("repoint");
    let to_v2 = resolve_generation(root, "previous", Some(&v1)).expect("resolve back");
    assert_eq!(
        oxide_fw_cache::read_build_id(&to_v2).expect("read"),
        v2,
        "the alternation holds: the swap left v2's copy retained"
    );
}
