//! Integration tests for the V2-F04 `experiments` config surface (§14 typed
//! config, §15.4 / V2-F04): the top-level `experiments` array parses through
//! the typed [`OxideConfig`], invalid entries fail the load with the DSP-110
//! diagnostic, and the JSON schema (`config_schema()`) declares the section
//! so editors can complete it.

use oxide_fw_config::{OxideConfig, config_schema, load_with_env};
use std::collections::BTreeMap;

const EMPTY_ENV: BTreeMap<String, String> = BTreeMap::new();

/// A minimal valid config with an `experiments` section.
fn config_json(experiments: &str) -> String {
    format!(
        r#"{{
            "framework": {{ "adapter": "react" }},
            "experiments": {experiments}
        }}"#
    )
}

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

#[test]
fn experiments_is_a_known_top_level_key() {
    let loaded = load_config(&config_json(
        r#"[{ "id": "hero", "traffic": 50, "sticky": { "sticky": "cookie", "cookieName": "oxide_hero" }, "variants": ["a", "b"] }]"#,
    ))
    .expect("a valid experiments section loads");
    assert_eq!(loaded.config.experiments.len(), 1, "the section parses");
}

#[test]
fn traffic_above_100_fails_with_dsp_110() {
    let err = load_config(&config_json(
        r#"[{ "id": "hero", "traffic": 150, "sticky": { "sticky": "ip-hash" }, "variants": ["a", "b"] }]"#,
    ))
    .expect_err("traffic > 100 must fail the load");
    assert_eq!(err.0.code, "DSP-110", "the §V2-F04 validation code");
}

#[test]
fn single_variant_fails_with_dsp_110() {
    let err = load_config(&config_json(
        r#"[{ "id": "hero", "traffic": 50, "sticky": { "sticky": "ip-hash" }, "variants": ["a"] }]"#,
    ))
    .expect_err("fewer than two variants must fail the load");
    assert_eq!(err.0.code, "DSP-110");
}

#[test]
fn empty_variants_fail_with_dsp_110() {
    let err = load_config(&config_json(
        r#"[{ "id": "hero", "traffic": 50, "sticky": { "sticky": "header", "headerName": "x-bucket" }, "variants": [] }]"#,
    ))
    .expect_err("no variants must fail the load");
    assert_eq!(err.0.code, "DSP-110");
}

#[test]
fn experiments_parse_every_sticky_kind() {
    let config: OxideConfig = serde_json::from_value(serde_json::json!({
        "experiments": [
            { "id": "hero", "traffic": 10, "sticky": { "sticky": "cookie", "cookieName": "oxide_hero" }, "variants": ["a", "b"], "routes": ["/pricing"] },
            { "id": "cta", "traffic": 25, "sticky": { "sticky": "header", "headerName": "x-bucket" }, "variants": ["x", "y"] },
            { "id": "geo", "traffic": 100, "sticky": { "sticky": "ip-hash" }, "variants": ["eu", "row"] }
        ]
    }))
    .expect("all three sticky kinds parse (camelCase fields)");
    assert_eq!(config.experiments.len(), 3);
    assert_eq!(config.experiments[0].id, "hero");
    assert_eq!(config.experiments[0].traffic, 10);
    assert_eq!(config.experiments[0].routes, vec!["/pricing".to_string()]);
    assert_eq!(
        config.experiments[1].sticky,
        oxide_fw_config::StickyConfig::Header {
            header_name: "x-bucket".to_string()
        }
    );
    assert_eq!(
        config.experiments[2].sticky,
        oxide_fw_config::StickyConfig::IpHash
    );
}

#[test]
fn experiments_default_to_empty() {
    let config: OxideConfig = serde_json::from_value(serde_json::json!({})).expect("default");
    assert!(
        config.experiments.is_empty(),
        "no experiments configured — the pre-F04 state the L0-key acceptance pins"
    );
}

#[test]
fn schema_declares_the_experiments_section() {
    let schema = config_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("schema has a properties object");
    let section = props
        .get("experiments")
        .and_then(|v| v.as_object())
        .expect("the experiments section is declared for editors");
    assert_eq!(
        section.get("type").and_then(|v| v.as_str()),
        Some("array"),
        "experiments is an array of experiment entries"
    );
}
