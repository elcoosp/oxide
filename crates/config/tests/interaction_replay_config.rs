//! Integration tests for the V2-F03 `interactionReplay` config surface
//! (Appendix V2.L: `interactionReplay?: boolean // F03, default true`):
//! the top-level boolean parses through the typed [`OxideConfig`] with the
//! spec's default (`true` — flag absent = enabled), a non-boolean value
//! fails the load with the DSP-114 diagnostic (the raw-`Value` validation
//! pass runs before the typed parse, so a `"false"` string gets the DSP
//! diagnostic instead of a serde type error), the typo diagnostic suggests
//! the camelCase key, and the JSON schema (`config_schema()`) declares it
//! so editors can complete it.
//!
//! Written FIRST (red): failed on the unknown-key diagnostic for
//! `interactionReplay` before the field, the DSP-114 validation and the
//! schema entry existed.

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

fn config_json(interaction_replay: &str) -> String {
    format!(
        r#"{{ "framework": {{ "adapter": "react" }}, "interactionReplay": {interaction_replay} }}"#
    )
}

// ------------------------------------------------------------------- defaults

#[test]
fn interaction_replay_absent_means_enabled() {
    // §V2-F03 deferral contract: "defaults stay ON (flag absent = enabled)"
    // — the client boot runtime honors `payload.interactionReplay === false`
    // only, so the typed default is `true`.
    let loaded = load_config(r#"{ "framework": { "adapter": "react" } }"#).expect("loads");
    assert!(
        loaded.config.interaction_replay,
        "the default build captures pre-hydration interactions"
    );
}

#[test]
fn interaction_replay_explicit_false_loads() {
    let loaded = load_config(&config_json("false")).expect("an explicit false loads");
    assert!(!loaded.config.interaction_replay);
}

#[test]
fn interaction_replay_explicit_true_loads() {
    let loaded = load_config(&config_json("true")).expect("an explicit true loads");
    assert!(loaded.config.interaction_replay);
}

// ------------------------------------------------------------------- DSP-114

fn expect_dsp_114(raw: &str, needle: &str) {
    let err = load_config(&config_json(raw)).expect_err("DSP-114 must reject this flag");
    assert_eq!(
        err.0.code, "DSP-114",
        "the diagnostic code: {}",
        err.0.message
    );
    assert!(
        err.0.message.contains(needle),
        "the diagnostic names the offending key ({needle}): {}",
        err.0.message
    );
    assert!(
        err.0.fix.is_some(),
        "the DSP-114 diagnostic carries a fix hint (the miette help text)"
    );
}

#[test]
fn dsp114_rejects_a_string_flag() {
    // The common mistake: quoting the boolean (a TS-config import shape).
    expect_dsp_114(r#""false""#, "interactionReplay");
}

#[test]
fn dsp114_rejects_a_numeric_flag() {
    expect_dsp_114("0", "interactionReplay");
    expect_dsp_114("1", "interactionReplay");
}

#[test]
fn dsp114_rejects_an_object_flag() {
    expect_dsp_114(r#"{ "enabled": false }"#, "interactionReplay");
}

#[test]
fn dsp114_rejects_a_null_flag() {
    expect_dsp_114("null", "interactionReplay");
}

// ------------------------------------------------------- typo diagnostics

#[test]
fn a_near_miss_key_gets_the_did_you_mean_suggestion() {
    let err = load_config(r#"{ "framework": { "adapter": "react" }, "interactionReplayy": true }"#)
        .expect_err("an unknown key must fail the load");
    assert!(
        err.0.message.contains("did you mean `interactionReplay`"),
        "the diagnostic suggests the camelCase key: {}",
        err.0.message
    );
}

// -------------------------------------------------------------- typed surface

#[test]
fn the_typed_config_field_exists_on_oxide_config() {
    // A deserialization round trip through the typed struct (not the load
    // path): OxideConfig carries `interactionReplay` next to `brownout`.
    let json = r#"{ "framework": { "adapter": "react" }, "interactionReplay": false }"#;
    let cfg: OxideConfig = serde_json::from_str(json).expect("typed parse");
    assert!(!cfg.interaction_replay);

    let default: OxideConfig =
        serde_json::from_str(r#"{ "framework": { "adapter": "react" } }"#).expect("default");
    assert!(default.interaction_replay);
}

// ------------------------------------------------------------------- schema

#[test]
fn the_schema_declares_interaction_replay() {
    let schema = config_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("schema has properties");
    let flag = props
        .get("interactionReplay")
        .and_then(|v| v.as_object())
        .expect("the schema declares interactionReplay");
    assert_eq!(
        flag.get("type").and_then(|v| v.as_str()),
        Some("boolean"),
        "the schema types the flag as a boolean"
    );
    assert_eq!(flag.get("default"), Some(&serde_json::json!(true)));
}
