//! Integration tests for the V2-F07 `stream` config surface (§14 typed
//! config, §V2-F07 wiring step 3: `stream.resume: { ttl: '60s',
//! maxBytesEach: '4mb', maxBytesTotal: '256mb' }`): the top-level `stream`
//! object parses through the typed [`OxideConfig`] with the spec's
//! defaults, invalid knobs fail the load with the DSP-112 diagnostic, and
//! the JSON schema (`config_schema()`) declares the section so editors can
//! complete it.
//!
//! Written FIRST (red): failed on unresolved-symbol errors before
//! `StreamConfig`/`StreamResumeConfig` and the DSP-112 validation existed.

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

fn config_json(stream: &str) -> String {
    format!(
        r#"{{
            "framework": {{ "adapter": "react" }},
            "stream": {stream}
        }}"#
    )
}

#[test]
fn stream_is_a_known_top_level_key_with_the_spec_defaults() {
    let loaded = load_config(&config_json("{}")).expect("an empty stream section loads");
    let resume = &loaded.config.stream.resume;
    assert_eq!(resume.ttl_secs, 60, "the spec's ttl: '60s' default");
    assert_eq!(
        resume.max_bytes_each, "4mb",
        "the spec's maxBytesEach default"
    );
    assert_eq!(
        resume.max_bytes_total, "256mb",
        "the spec's maxBytesTotal default"
    );
}

#[test]
fn stream_absent_means_the_defaults_still_parse() {
    let loaded = load_config(r#"{ "framework": { "adapter": "react" } }"#).expect("loads");
    assert_eq!(loaded.config.stream.resume.ttl_secs, 60);
    assert_eq!(loaded.config.stream.resume.max_bytes_each, "4mb");
}

#[test]
fn stream_knobs_parse_camel_case() {
    let loaded = load_config(&config_json(
        r#"{ "resume": { "ttlSecs": 30, "maxBytesEach": "1mb", "maxBytesTotal": "8mb" } }"#,
    ))
    .expect("a full stream section loads");
    let resume = &loaded.config.stream.resume;
    assert_eq!(resume.ttl_secs, 30);
    assert_eq!(resume.max_bytes_each, "1mb");
    assert_eq!(resume.max_bytes_total, "8mb");
}

#[test]
fn unknown_stream_keys_are_rejected() {
    let err = load_config(&config_json(r#"{ "resume": { "maxsize": "1mb" } }"#))
        .expect_err("deny_unknown_fields guards the section");
    assert!(
        err.0.message.contains("maxsize"),
        "the diagnostic names the offending key: {}",
        err.0.message
    );
}

#[test]
fn zero_ttl_fails_with_dsp_112() {
    let err = load_config(&config_json(r#"{ "resume": { "ttlSecs": 0 } }"#))
        .expect_err("a 0s ttl expires every buffer before the reattach arrives");
    assert_eq!(err.0.code, "DSP-112", "the §V2-F07 load-validation code");
}

#[test]
fn zero_max_bytes_each_fails_with_dsp_112() {
    let err = load_config(&config_json(r#"{ "resume": { "maxBytesEach": "0b" } }"#))
        .expect_err("a 0-byte per-stream bound disables replay entirely");
    assert_eq!(err.0.code, "DSP-112");
}

#[test]
fn max_bytes_total_below_max_bytes_each_fails_with_dsp_112() {
    let err = load_config(&config_json(
        r#"{ "resume": { "maxBytesEach": "4mb", "maxBytesTotal": "1kb" } }"#,
    ))
    .expect_err("the total bound cannot be smaller than the per-stream bound");
    assert_eq!(err.0.code, "DSP-112");
}

#[test]
fn invalid_size_literal_fails_with_cfg_size() {
    let err = load_config(&config_json(r#"{ "resume": { "maxBytesEach": "huge" } }"#))
        .expect_err("the size-literal grammar is the §14 one");
    assert_eq!(err.0.code, "CFG-SIZE");
}

#[test]
fn the_schema_declares_the_stream_section() {
    let schema = config_schema();
    let props = schema
        .get("properties")
        .and_then(|p| p.get("stream"))
        .expect("the stream section is declared");
    let resume = props
        .get("properties")
        .and_then(|p| p.get("resume"))
        .and_then(|p| p.get("properties"))
        .expect("the nested resume object is declared");
    for knob in ["ttlSecs", "maxBytesEach", "maxBytesTotal"] {
        assert!(
            resume.get(knob).is_some(),
            "the schema declares resume.{knob}"
        );
    }
    assert!(
        props.get("description").is_some(),
        "the section carries a description for editors"
    );
}

/// The typed surface: `OxideConfig::default()` is inert-and-valid, and the
/// section round-trips serde without loss (the CLI's typed access path).
#[test]
fn the_typed_config_defaults_and_round_trips() {
    let cfg = OxideConfig::default();
    assert_eq!(cfg.stream.resume.ttl_secs, 60);
    let json = serde_json::to_string(&cfg).expect("serializes");
    let back: OxideConfig = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(back.stream.resume.max_bytes_each, "4mb");
}
