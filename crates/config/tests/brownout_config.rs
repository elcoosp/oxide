//! Integration tests for the V2-F12 `brownout` config surface (§14 typed
//! config, §V2-F12: `brownout: { enabled, staleCeiling, thresholds:
//! { queueWaitMs, loopLagMs, rssMb }, hedge: { enabled, afterFactor } }`):
//! the top-level `brownout` object parses through the typed [`OxideConfig`]
//! with the spec's defaults (off, "10x" ceiling, the §V2-F12 code block's
//! threshold ladders, the hedger opt-in), invalid knobs fail the load with
//! the DSP-113 diagnostic, and the JSON schema (`config_schema()`) declares
//! the section so editors can complete it.
//!
//! Written FIRST (red): failed on unresolved-symbol errors before
//! `BrownoutConfig`/`HedgeConfig` and the DSP-113 validation existed.

use std::collections::BTreeMap;

use oxide_fw_config::{OxideConfig, config_schema, load_with_env};

const EMPTY_ENV: BTreeMap<String, String> = BTreeMap::new();

/// Write `oxide.config.json` into a temp project dir and load it.
fn load_config(
    body: &str,
) -> Result<oxide_fw_config::LoadedConfig, oxide_fw_diagnostics::OxideError> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("oxide.config.json");
    std::fs::write(&path, body).expect("write config");
    let dir = tmp.path().to_path_buf();
    load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV)
}

fn config_json(brownout: &str) -> String {
    format!(
        r#"{{
            "framework": {{ "adapter": "react" }},
            "brownout": {brownout}
        }}"#
    )
}

// ------------------------------------------------------------------- defaults

#[test]
fn brownout_absent_means_the_defaults_still_parse() {
    let loaded = load_config(r#"{ "framework": { "adapter": "react" } }"#).expect("loads");
    let b = &loaded.config.brownout;
    assert!(!b.enabled, "brownout is opt-in: the default build is inert");
    assert_eq!(b.stale_ceiling, "10x", "the spec's staleCeiling default");
}

#[test]
fn brownout_defaults_match_the_spec_code_block() {
    let loaded = load_config(&config_json("{}")).expect("an empty brownout section loads");
    let b = &loaded.config.brownout;
    assert!(!b.enabled);
    assert_eq!(b.stale_ceiling, "10x");
    // The §V2-F12 Thresholds::default ladders, in milliseconds.
    assert_eq!(b.thresholds.queue_wait_ms, [50, 200, 500]);
    assert_eq!(b.thresholds.loop_lag_ms, [20, 100, 300]);
    // RSS breaches are disabled by default: 0 means "no RSS gate" (the
    // wiring maps it to the spec's u64::MAX never-breach sentinel).
    assert_eq!(b.thresholds.rss_mb, [0, 0, 0]);
    assert!(!b.hedge.enabled, "the hedger is opt-in");
    assert!(
        (b.hedge.after_factor - 2.0).abs() < f64::EPSILON,
        "the spec's afterFactor 2.0"
    );
}

#[test]
fn brownout_knobs_parse_camel_case() {
    let loaded = load_config(&config_json(
        r#"{
            "enabled": true,
            "staleCeiling": "5x",
            "thresholds": { "queueWaitMs": [10, 50, 100], "loopLagMs": [5, 40, 90], "rssMb": [512, 1024, 2048] },
            "hedge": { "enabled": true, "afterFactor": 3.5 }
        }"#,
    ))
    .expect("a full brownout section loads");
    let b = &loaded.config.brownout;
    assert!(b.enabled);
    assert_eq!(b.stale_ceiling, "5x");
    assert_eq!(b.thresholds.queue_wait_ms, [10, 50, 100]);
    assert_eq!(b.thresholds.loop_lag_ms, [5, 40, 90]);
    assert_eq!(b.thresholds.rss_mb, [512, 1024, 2048]);
    assert!(b.hedge.enabled);
    assert!((b.hedge.after_factor - 3.5).abs() < f64::EPSILON);
}

#[test]
fn unknown_brownout_keys_are_rejected() {
    let err = load_config(&config_json(r#"{ "ceiling": "10x" }"#))
        .expect_err("deny_unknown_fields guards the section");
    assert!(
        err.0.message.contains("ceiling"),
        "the diagnostic names the offending key: {}",
        err.0.message
    );
}

// ----------------------------------------------------------------- DSP-113

fn expect_dsp_113(body: &str, needle: &str) {
    let err = load_config(&config_json(body)).expect_err("DSP-113 must reject this section");
    assert_eq!(
        err.0.code, "DSP-113",
        "the diagnostic code: {}",
        err.0.message
    );
    assert!(
        err.0.message.contains(needle),
        "the diagnostic names the offending knob ({needle}): {}",
        err.0.message
    );
}

#[test]
fn dsp113_rejects_a_non_ascending_threshold_ladder() {
    expect_dsp_113(
        r#"{ "thresholds": { "queueWaitMs": [500, 200, 50] } }"#,
        "queueWaitMs",
    );
}

#[test]
fn dsp113_rejects_a_zero_or_negative_stale_ceiling() {
    expect_dsp_113(r#"{ "staleCeiling": "0x" }"#, "staleCeiling");
    expect_dsp_113(r#"{ "staleCeiling": "-3x" }"#, "staleCeiling");
}

#[test]
fn dsp113_rejects_an_unparseable_stale_ceiling() {
    expect_dsp_113(r#"{ "staleCeiling": "soon" }"#, "staleCeiling");
}

#[test]
fn dsp113_rejects_a_hedge_factor_at_or_below_one() {
    expect_dsp_113(r#"{ "hedge": { "afterFactor": 1.0 } }"#, "afterFactor");
    expect_dsp_113(r#"{ "hedge": { "afterFactor": 0.5 } }"#, "afterFactor");
}

#[test]
fn dsp113_rejects_a_non_object_section() {
    expect_dsp_113("42", "brownout");
}

#[test]
fn dsp113_rejects_wrong_length_threshold_ladders() {
    expect_dsp_113(
        r#"{ "thresholds": { "queueWaitMs": [50, 200] } }"#,
        "queueWaitMs",
    );
    expect_dsp_113(
        r#"{ "thresholds": { "loopLagMs": [20, 100, 300, 400] } }"#,
        "loopLagMs",
    );
}

#[test]
fn dsp113_allows_flat_and_disabled_ladders() {
    // Equal rungs are legal (a single escalation jump); all-zero rssMb is
    // the documented "no RSS gate" spelling.
    let loaded = load_config(&config_json(
        r#"{ "enabled": true, "thresholds": { "queueWaitMs": [100, 100, 100], "rssMb": [0, 0, 0] } }"#,
    ))
    .expect("flat ladders and disabled rss load");
    assert!(loaded.config.brownout.enabled);
}

// ------------------------------------------------------------------- helpers

#[test]
fn stale_ceiling_multiplier_parses_plain_and_suffixed() {
    use oxide_fw_config::parse_stale_ceiling;
    assert!((parse_stale_ceiling("10x").expect("10x") - 10.0).abs() < f64::EPSILON);
    assert!((parse_stale_ceiling("10").expect("10") - 10.0).abs() < f64::EPSILON);
    assert!((parse_stale_ceiling("1.5x").expect("1.5x") - 1.5).abs() < f64::EPSILON);
    assert!(
        parse_stale_ceiling("x").is_none(),
        "a bare suffix is not a multiplier"
    );
    assert!(parse_stale_ceiling("").is_none());
    assert!(
        parse_stale_ceiling("0x").is_none(),
        "zero is rejected by the parser too"
    );
}

// ------------------------------------------------------------------- schema

#[test]
fn the_schema_declares_the_brownout_section() {
    let schema = config_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("schema has properties");
    let brownout = props
        .get("brownout")
        .and_then(|v| v.as_object())
        .expect("the schema declares brownout");
    let fields = brownout
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("brownout has properties");
    for key in ["enabled", "staleCeiling", "thresholds", "hedge"] {
        assert!(
            fields.contains_key(key),
            "schema brownout is missing {key:?}"
        );
    }
    let hedge = fields
        .get("hedge")
        .and_then(|v| v.as_object())
        .expect("hedge declared");
    let hedge_fields = hedge
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("hedge has properties");
    for key in ["enabled", "afterFactor"] {
        assert!(
            hedge_fields.contains_key(key),
            "schema hedge is missing {key:?}"
        );
    }
}

// -------------------------------------------------------------- typed surface

#[test]
fn the_typed_config_field_exists_on_oxide_config() {
    // A deserialization round trip through the typed struct (not the load
    // path): OxideConfig carries `brownout` next to `live` and `stream`.
    let json = r#"{ "framework": { "adapter": "react" }, "brownout": { "enabled": true } }"#;
    let cfg: OxideConfig = serde_json::from_str(json).expect("typed parse");
    assert!(cfg.brownout.enabled);
}
