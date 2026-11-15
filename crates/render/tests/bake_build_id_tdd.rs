//! TDD pins for the V2-A04 prerendered-shell build-id bake: the emit tail
//! rewrites the prerendered shells' hydration bootstrap to carry the
//! CURRENT build id (the four-field shape) so a prerendered page echoes an
//! ACCEPTED id on the data plane instead of riding the headerless
//! allowance — without a 409 loop (the deviation recorded in
//! `docs/deviations.md`: the covered set's tail writers run post-prerender,
//! so the bake must happen at the post-tail point, on the id the stamp
//! computed).
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `bake_build_id` existed.

use oxide_fw_render::{bake_build_id, hydration_bootstrap, hydration_bootstrap_with_build};

const B64: &str = "eyJkIjoxfQ==";
const NONCE: &str = "n0nce";
const ID: &str = "deadbeef00000001";

#[test]
fn bake_reproduces_the_four_field_bootstrap() {
    // The legacy shell (the three-field bootstrap the prerender pass
    // writes) bakes to EXACTLY the four-field A04 shape — no re-render,
    // no drift between the served and prerendered contracts.
    let shell = hydration_bootstrap(B64, "routes/index", NONCE);
    let expected = hydration_bootstrap_with_build(B64, "routes/index", NONCE, ID);
    assert_eq!(
        bake_build_id(&shell, ID).expect("the bootstrap marker is present"),
        expected,
        "the bake is the A04 template, byte for byte"
    );
}

#[test]
fn bake_is_idempotent() {
    let baked = hydration_bootstrap_with_build(B64, "routes/index", NONCE, ID);
    assert!(
        bake_build_id(&baked, ID).is_none(),
        "a shell that already carries a build field is left untouched"
    );
}

#[test]
fn bake_upgrades_a_stale_id_to_the_current_one() {
    // A shell from an older build (baked with a different id) re-bakes to
    // the CURRENT id — an incremental rebuild must not leave the old id
    // behind (the serve pool would reject it inside the window's tail).
    let stale = hydration_bootstrap_with_build(B64, "routes/index", NONCE, "aaaaaaaaaaaaaaaa");
    let out = bake_build_id(&stale, ID).expect("a stale id is replaced");
    let expected = hydration_bootstrap_with_build(B64, "routes/index", NONCE, ID);
    assert_eq!(out, expected);
}

#[test]
fn bake_bakes_every_bootstrap_marker_and_absent_marker_is_none() {
    // A page with several shells (a fixture-embedded snippet) bakes all of
    // them; HTML without the `window.__oxide` marker is a no-op (the
    // caller skips the write).
    let shell = hydration_bootstrap(B64, "routes/index", NONCE);
    let two = format!("<html>{shell}{shell}</html>");
    let out = bake_build_id(&two, ID).expect("both markers bake");
    assert_eq!(
        out.matches(&format!("build:\"{ID}\"")).count(),
        2,
        "every bootstrap in the document carries the id"
    );
    assert!(
        bake_build_id("<html><body>no oxide data here</body></html>", ID).is_none(),
        "a document without the bootstrap marker is a no-op"
    );
}

#[test]
fn bake_sanitizes_the_id_like_the_template() {
    // A non-hex id goes through the same percent-encoding
    // `sanitize_route_id` applies in the templates — the bake never
    // writes a byte the JS string literal cannot carry.
    let shell = hydration_bootstrap(B64, "routes/index", NONCE);
    let out = bake_build_id(&shell, "id with spaces").expect("the marker is present");
    assert!(
        !out.contains("build:\"id with spaces\""),
        "the raw id never lands in the script"
    );
    assert!(out.contains("build:\"id%20with%20spaces\""));
}
