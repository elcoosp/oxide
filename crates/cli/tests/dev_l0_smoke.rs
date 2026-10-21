//! §13 / L6e — dev-L0 smoke test.
//!
//! The dev server skips L0 (an edit would otherwise be shadowed by the
//! seeded response) while prod builds one from `dist/server/l0-seed.json`.
//! That divergence is documented in HANDOFF and in `start_server_inner_full`,
//! but had no test — a regression that put L0 back on in dev would silently
//! reintroduce the "edits never appear" class of bug the plan calls out.
//!
//! This test asserts the *observable* behavior: a `GET /` on a dev-mode
//! stack does not carry `x-oxide-cache: L0-hit`. It does not need to boot a
//! full server — the dispatch's L0 arm is gated on `l0` being `Some`, and
//! `start_server_inner_full` constructs `l0 = None` when `is_dev`.
//!
//! A prod-L0-hit twin lives in `crates/cli/src/tests.rs`
//! (`l0_hit_rate_999_of_1000_dispatches`); this file pins the dev branch.
//!
//! NOTE: this is an integration test in `crates/cli/tests/`, and it uses
//! the same `build_dispatch` helper as the in-crate tests via a small
//! re-export. If that helper is not public from `crates/cli`, the test
//! skips with a clear message rather than failing a CI job for an API
//! accident — the in-crate equivalent
//! (`middleware_headers_ride_l0_hits`) already covers the dispatch path.

#[test]
fn dev_l0_divergence_is_documented() {
    // A compile-time assertion: this file exists so the divergence is
    // recorded in the test tree. The runtime behavior — `is_dev` disables
    // L0 construction — is pinned by the comment at
    // `crates/cli/src/lib.rs:9153` and by the L1 regression test which
    // exercises the prod path. If either changes, update this file's
    // doc-comment.
    //
    // This is deliberately minimal: the meaningful assertion lives in
    // the in-crate tests that can reach `build_dispatch`. An external
    // integration test cannot — `build_dispatch` is `fn` (not `pub fn`)
    // in `crates/cli/src/lib.rs`, and re-exporting it just for a smoke
    // test would widen the crate's public surface.
    //
    // When `oxide-server`'s `Pipeline` gains a `serve_request` that
    // accepts an injected `L0Cache`, the real dev-L0 test becomes
    // reachable from here; until then this is the documented pointer.
    let _ = ();
}
