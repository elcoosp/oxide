//! TDD pins for the V2-F03 interaction-replay advertisement in the
//! hydration bootstrap (the F03 deferral contract in `docs/deviations.md`:
//! "the boot runtime honors `payload.interactionReplay === false` today, so
//! emitting the field is the only remaining change and defaults stay ON
//! (flag absent = enabled)").
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `hydration_bootstrap_with_replay` and
//! `Injector::with_interaction_replay` existed. The F05 contract carries
//! over: the pre-F03 spellings stay byte-identical — an enabled (or
//! absent) flag never changes the emitted bytes, so the V8-linked CLI call
//! sites compile unchanged until the A01-phase-3 wiring lands.

use oxide_fw_render::{
    HeadTags, Injector, hydration_bootstrap_with_build, hydration_bootstrap_with_live,
    hydration_bootstrap_with_replay,
};

const B64: &str = "eyJkIjoxfQ==";
const NONCE: &str = "n0nce";

#[test]
fn replay_disabled_bootstrap_emits_the_flag() {
    // `interactionReplay: false` sits between the optional `live` field and
    // the `data` reference — the boot runtime's `!== false` check reads
    // exactly this spelling (packages/core boot.ts).
    let html = hydration_bootstrap_with_replay(
        B64,
        "routes/index",
        NONCE,
        "deadbeef00000001",
        false,
        false,
    );
    assert_eq!(
        html,
        format!(
            "<script type='application/oxide-data' nonce='{NONCE}'>{B64}</script><script nonce='{NONCE}'>window.__oxide={{route:\"routes/index\",build:\"deadbeef00000001\",interactionReplay:false,data:document.currentScript.previousElementSibling.textContent}}</script>"
        )
    );
}

#[test]
fn replay_disabled_live_bootstrap_composes_with_the_live_field() {
    // A live route with replay disabled carries both fields, in field
    // order: route, build, live, interactionReplay, data.
    let html = hydration_bootstrap_with_replay(
        B64,
        "routes/index",
        NONCE,
        "deadbeef00000001",
        true,
        false,
    );
    assert_eq!(
        html,
        format!(
            "<script type='application/oxide-data' nonce='{NONCE}'>{B64}</script><script nonce='{NONCE}'>window.__oxide={{route:\"routes/index\",build:\"deadbeef00000001\",live:true,interactionReplay:false,data:document.currentScript.previousElementSibling.textContent}}</script>"
        )
    );
}

#[test]
fn replay_enabled_is_byte_identical_to_the_pre_f03_shapes() {
    // Flag absent = enabled: `interaction_replay: true` must not change a
    // byte, for either live shape (the additive-substrate contract).
    assert_eq!(
        hydration_bootstrap_with_replay(
            B64,
            "routes/index",
            NONCE,
            "deadbeef00000001",
            false,
            true
        ),
        hydration_bootstrap_with_build(B64, "routes/index", NONCE, "deadbeef00000001")
    );
    assert_eq!(
        hydration_bootstrap_with_replay(B64, "routes/index", NONCE, "deadbeef00000001", true, true),
        hydration_bootstrap_with_live(B64, "routes/index", NONCE, "deadbeef00000001", true)
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
fn injector_with_replay_disabled_emits_the_flag() {
    let mut inj = injector()
        .with_build_id("deadbeef00000001")
        .with_interaction_replay(false);
    inj.odf_payload_b64 = Some(B64.to_string());
    let html = inj
        .inject_into_html("<!doctype html><html><head></head><body><h1>hi</h1></body></html>")
        .expect("inject");
    assert!(
        html.contains("interactionReplay:false"),
        "the config opt-out reaches the emitted page: {html}"
    );
    assert!(html.contains("build:\"deadbeef00000001\""), "{html}");
}

#[test]
fn injector_default_keeps_the_pre_f03_bootstrap() {
    // The default Injector never emits the field (flag absent = enabled).
    let mut inj = injector().with_build_id("deadbeef00000001");
    inj.odf_payload_b64 = Some(B64.to_string());
    let html = inj
        .inject_into_html("<!doctype html><html><head></head><body><h1>hi</h1></body></html>")
        .expect("inject");
    assert!(
        !html.contains("interactionReplay"),
        "no interactionReplay field by default: {html}"
    );
}
