//! TDD pins for the V2-F13 hydration-bootstrap advertisement: the seventh
//! `window.__oxide` field (`prefetch:true`) tells the client runtime the
//! server's nav-graph endpoint is live, so the learned prefetcher may
//! consult `GET /_oxide/nav`.
//!
//! The additive contract (the F05/F03 pattern): `prefetch: false` — the
//! default — keeps every emitted byte identical to the pre-F13 spellings;
//! only `true` inserts the field. Written BEFORE the implementation; every
//! pin imports public API only.

use oxide_fw_render::{
    HeadTags, Injector, hydration_bootstrap, hydration_bootstrap_prefetch_no_build,
    hydration_bootstrap_with_prefetch, hydration_bootstrap_with_replay,
};

const ODF: &str = "aaQ"; // an opaque base64 payload (never decoded here)

#[test]
fn prefetch_false_is_byte_identical_to_the_pre_f13_shapes() {
    // With build id + live + replay flags at their defaults.
    let baseline = hydration_bootstrap_with_replay(ODF, "/r", "n1", "b1", false, true);
    let with_fn = hydration_bootstrap_with_prefetch(ODF, "/r", "n1", "b1", false, true, false);
    assert_eq!(
        with_fn, baseline,
        "prefetch:false delegates to the pre-F13 spelling"
    );
    // ... and to the plain three-field shape when no build id rides.
    let plain = hydration_bootstrap(ODF, "/r", "n1");
    let no_build = hydration_bootstrap_prefetch_no_build(ODF, "/r", "n1", false);
    assert_eq!(no_build, plain, "no-build prefetch:false is byte-identical");
    assert!(
        !plain.contains("prefetch"),
        "the baseline shape never advertises prefetch"
    );
}

#[test]
fn prefetch_true_inserts_the_seventh_field() {
    let out = hydration_bootstrap_with_prefetch(ODF, "/r", "n1", "b1", true, true, true);
    assert!(
        out.contains("prefetch:true"),
        "the bootstrap advertises the nav endpoint (got: {out})"
    );
    // Field order: route, build, live, interactionReplay(opt), prefetch, data.
    let route_idx = out.find("route:").expect("route field");
    let build_idx = out.find("build:").expect("build field");
    let live_idx = out.find("live:true").expect("live field");
    let prefetch_idx = out.find("prefetch:true").expect("prefetch field");
    let data_idx = out.find("data:").expect("data field");
    assert!(route_idx < build_idx);
    assert!(build_idx < live_idx);
    assert!(live_idx < prefetch_idx);
    assert!(prefetch_idx < data_idx);
}

#[test]
fn prefetch_field_rides_the_live_and_replay_variants() {
    // interactionReplay:false (the sixth field) + prefetch:true (the seventh).
    let out = hydration_bootstrap_with_prefetch(ODF, "/r", "n1", "b1", false, false, true);
    assert!(out.contains("interactionReplay:false"));
    assert!(out.contains("prefetch:true"));
    // The interactionReplay field precedes prefetch.
    let replay_idx = out.find("interactionReplay:false").expect("replay field");
    let prefetch_idx = out.find("prefetch:true").expect("prefetch field");
    assert!(replay_idx < prefetch_idx);
}

#[test]
fn prefetch_no_build_advertises_without_a_build_id() {
    // The placeholder path (no build id) gains the field too — the
    // two-field-plus-prefetch shape between the plain three-field and the
    // full A04 chain.
    let out = hydration_bootstrap_prefetch_no_build(ODF, "/r", "n1", true);
    assert!(out.contains("prefetch:true"));
    let route_idx = out.find("route:").expect("route field");
    let prefetch_idx = out.find("prefetch:true").expect("prefetch field");
    let data_idx = out.find("data:").expect("data field");
    assert!(route_idx < prefetch_idx);
    assert!(prefetch_idx < data_idx);
    assert!(!out.contains("build:"), "no build id => no build field");
}

#[test]
fn injector_with_prefetch_flips_the_bootstrap_emission() {
    let make = |prefetch: bool| {
        Injector::new(HeadTags::default(), "n1".into(), "/r".into())
            .with_build_id("b1")
            .with_prefetch(prefetch)
    };
    let mut on = make(true);
    on.odf_payload_b64 = Some(ODF.into());
    let html_on = on
        .inject_into_html("<html><head></head><body></body></html>")
        .expect("inject");
    assert!(
        html_on.contains("prefetch:true"),
        "the injector emits the advertisement when set"
    );

    let mut off = make(false);
    off.odf_payload_b64 = Some(ODF.into());
    let html_off = off
        .inject_into_html("<html><head></head><body></body></html>")
        .expect("inject");
    assert!(
        !html_off.contains("prefetch"),
        "the default keeps the pre-F13 bytes"
    );
}

#[test]
fn injector_prefetch_reaches_the_no_build_path() {
    // No build id: the injector falls back to the plain shape — with
    // prefetch the no-build advertisement must still emit.
    let mut inj = Injector::new(HeadTags::default(), "n1".into(), "/r".into()).with_prefetch(true);
    inj.odf_payload_b64 = Some(ODF.into());
    let html = inj
        .inject_into_html("<html><head></head><body></body></html>")
        .expect("inject");
    assert!(html.contains("prefetch:true"));
    assert!(!html.contains("build:"));
}

#[test]
fn injector_default_prefetch_is_false() {
    let inj = Injector::new(HeadTags::default(), "n1".into(), "/r".into());
    assert!(!inj.prefetch, "the field defaults to the pre-F13 behavior");
}
