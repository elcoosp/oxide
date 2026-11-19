//! Integration tests for the V2-F48 `cache.healthMetrics` config
//! surface (§20 / the F32 deviations log's named future consumer: the
//! posture pull's gauge exposition reaches the §20 production scrape
//! behind the operator's opt-in): the key parses through the typed
//! [`OxideConfig`], the default stays `None` (the F40 byte-shape never
//! moves behind a config without the key), both spellings are legal
//! (`true` and `false` — the baseline is a real choice, not a one-way
//! opt-in), and the JSON schema (`config_schema()`) declares the knob
//! so editors can complete it.
//!
//! Written FIRST (red): failed on the unresolved `health_metrics` field
//! before `CacheConfig::health_metrics` and the schema property existed.

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

fn config_json(cache: &str) -> String {
    format!(
        r#"{{
            "framework": {{ "adapter": "react" }},
            "cache": {cache}
        }}"#
    )
}

/// The operator's opt-in lands: `cache.healthMetrics: true` parses
/// through the typed config and rides the boot wire.
#[test]
fn cache_health_metrics_parses_from_the_config() {
    let loaded = load_config(&config_json(r#"{ "healthMetrics": true }"#))
        .expect("a cache section with healthMetrics loads");
    assert_eq!(
        loaded.config.cache.health_metrics,
        Some(true),
        "the project's committed health opt-in"
    );
}

/// A committed `false` is a real choice too: the baseline can pin the
/// plane's posture off (the knob is not a one-way opt-in).
#[test]
fn cache_health_metrics_false_is_a_legal_choice() {
    let loaded = load_config(&config_json(r#"{ "healthMetrics": false }"#))
        .expect("healthMetrics false loads");
    assert_eq!(
        loaded.config.cache.health_metrics,
        Some(false),
        "the project's committed-off shape"
    );
}

/// Without the key the field stays `None` — the default-off shape
/// answers at the wire and the boot shape is byte-identical (the F40
/// doctrine).
#[test]
fn cache_health_metrics_defaults_to_none() {
    let loaded = load_config(&config_json("{}")).expect("an empty cache section loads");
    assert_eq!(
        loaded.config.cache.health_metrics, None,
        "the key absent keeps the default-off shape"
    );
}

/// The schema declares the knob: type boolean, default false — so
/// editors complete `healthMetrics` and the docs page renders it.
#[test]
fn cache_schema_declares_health_metrics() {
    let schema = config_schema();
    let cache = &schema["properties"]["cache"]["properties"];
    let health = cache
        .get("healthMetrics")
        .expect("cache schema declares healthMetrics");
    assert_eq!(health["type"], "boolean", "the knob is a boolean");
    assert_eq!(health["default"], false, "the default-off shape");
}

/// The typed struct carries the key under the camelCase spelling the
/// schema documents (`deny_unknown_fields` would refuse the section
/// otherwise — the parse pins above hold the same contract).
#[test]
fn cache_config_deserializes_camel_case_health_metrics() {
    let body = r#"{
        "framework": { "adapter": "react" },
        "cache": { "healthMetrics": true }
    }"#;
    let cfg: OxideConfig = serde_json::from_str(body).expect("camelCase key deserializes");
    assert_eq!(
        cfg.cache.health_metrics,
        Some(true),
        "the schema's camelCase spelling rides the typed struct"
    );
}
