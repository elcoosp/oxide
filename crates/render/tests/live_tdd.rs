//! TDD pins for the V2-F05 live advertisement in the hydration bootstrap
//! (spec §V2-F05 step 1: "`hydration_bootstrap` adds `live:true` to
//! `window.__oxide` for such routes").
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `hydration_bootstrap_with_live` and `Injector::with_live`
//! existed. The A04 contract carries over: the pre-F05 spellings
//! (`hydration_bootstrap`, `hydration_bootstrap_with_build`) stay
//! byte-identical — the V8-linked CLI call sites compile unchanged until
//! the A01-phase-3 wiring lands.

use oxide_fw_render::{
    HeadTags, Injector, hydration_bootstrap, hydration_bootstrap_with_build,
    hydration_bootstrap_with_live,
};

const B64: &str = "eyJkIjoxfQ==";
const NONCE: &str = "n0nce";

#[test]
fn pre_f05_bootstrap_spellings_are_unchanged() {
    // The three- and four-field variants keep their byte-exact shapes —
    // the client keys on the exact `window.__oxide` contract.
    let payload = format!("<script type='application/oxide-data' nonce='{NONCE}'>{B64}</script>");
    assert_eq!(
        hydration_bootstrap(B64, "routes/index", NONCE),
        format!(
            "{payload}<script nonce='{NONCE}'>window.__oxide={{route:\"routes/index\",data:document.currentScript.previousElementSibling.textContent}}</script>"
        )
    );
    assert_eq!(
        hydration_bootstrap_with_build(B64, "routes/index", NONCE, "deadbeef00000001"),
        format!(
            "{payload}<script nonce='{NONCE}'>window.__oxide={{route:\"routes/index\",build:\"deadbeef00000001\",data:document.currentScript.previousElementSibling.textContent}}</script>"
        )
    );
}

#[test]
fn live_bootstrap_advertises_live_true() {
    let html = hydration_bootstrap_with_live(B64, "routes/index", NONCE, "deadbeef00000001", true);
    assert_eq!(
        html,
        format!(
            "<script type='application/oxide-data' nonce='{NONCE}'>{B64}</script><script nonce='{NONCE}'>window.__oxide={{route:\"routes/index\",build:\"deadbeef00000001\",live:true,data:document.currentScript.previousElementSibling.textContent}}</script>"
        )
    );
}

#[test]
fn live_false_equals_the_build_only_bootstrap() {
    // `with_live(false)` must be byte-identical to the A04 four-field
    // bootstrap — the additive flag changes nothing when not opted in.
    assert_eq!(
        hydration_bootstrap_with_live(B64, "routes/index", NONCE, "deadbeef00000001", false),
        hydration_bootstrap_with_build(B64, "routes/index", NONCE, "deadbeef00000001")
    );
}

fn injector() -> Injector {
    Injector::new(
        HeadTags::default(),
        NONCE.to_string(),
        "routes/index".to_string(),
    )
}

#[test]
fn injector_with_live_emits_the_live_bootstrap() {
    let mut inj = injector().with_build_id("deadbeef00000001").with_live(true);
    inj.odf_payload_b64 = Some(B64.to_string());
    let html = inj
        .inject_into_html("<!doctype html><html><head></head><body><h1>hi</h1></body></html>")
        .expect("inject");
    assert!(
        html.contains("live:true"),
        "the live opt-in reaches the emitted page: {html}"
    );
    assert!(html.contains("build:\"deadbeef00000001\""), "{html}");
}

#[test]
fn injector_without_live_keeps_the_a04_bootstrap() {
    let mut inj = injector().with_build_id("deadbeef00000001");
    inj.odf_payload_b64 = Some(B64.to_string());
    let html = inj
        .inject_into_html("<!doctype html><html><head></head><body><h1>hi</h1></body></html>")
        .expect("inject");
    assert!(!html.contains("live:"), "no live field by default: {html}");
}
