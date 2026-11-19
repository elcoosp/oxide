//! Integration tests for the V2-F43 `cache.hotKeyTop` config surface
//! (§20 / the F41 deviations log's named "future consumer": the project's
//! committed baseline for the §20 scrape's hot-key top window): the key
//! parses through the typed [`OxideConfig`], the default stays `None`
//! (the F40 shape never moves behind a config without the key), `0` is
//! legal (window 0 renders the bare family pair — the F41 posture), a
//! negative value fails the load, and the JSON schema
//! (`config_schema()`) declares the knob so editors can complete it.
//!
//! Written FIRST (red): failed on the unresolved `hot_key_top` field
//! before `CacheConfig::hot_key_top` and the schema property existed.

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

/// The operator's baseline lands: `cache.hotKeyTop` parses through the
/// typed config and rides the boot wire.
#[test]
fn cache_hot_key_top_parses_from_the_config() {
    let loaded = load_config(&config_json(r#"{ "hotKeyTop": 50 }"#))
        .expect("a cache section with hotKeyTop loads");
    assert_eq!(
        loaded.config.cache.hot_key_top,
        Some(50),
        "the project's committed window baseline"
    );
}

/// Without the key the field stays `None` — the F40 default window
/// answers at the wire and the boot shape is byte-identical.
#[test]
fn cache_hot_key_top_defaults_to_none() {
    let loaded = load_config(&config_json("{}")).expect("an empty cache section loads");
    assert_eq!(
        loaded.config.cache.hot_key_top, None,
        "the key absent keeps the F40 shape"
    );
}

/// `0` is legal: window 0 renders the bare family pair (the F41
/// posture), so the config layer must not floor the knob at 1.
#[test]
fn cache_hot_key_top_zero_is_a_legal_window() {
    let loaded = load_config(&config_json(r#"{ "hotKeyTop": 0 }"#)).expect("hotKeyTop 0 loads");
    assert_eq!(
        loaded.config.cache.hot_key_top,
        Some(0),
        "the bare-family window is a committed choice"
    );
}

/// A negative window fails the load (an integer the resolver can never
/// answer is a config error, not a silent default).
#[test]
fn cache_hot_key_top_negative_fails_the_load() {
    let result = load_config(&config_json(r#"{ "hotKeyTop": -1 }"#));
    assert!(result.is_err(), "a negative window is a config error");
}

/// The schema declares the knob: type integer, minimum 0 — so editors
/// complete `hotKeyTop` and the docs page renders it.
#[test]
fn cache_schema_declares_hot_key_top() {
    let schema = config_schema();
    let cache = &schema["properties"]["cache"]["properties"];
    let hot = cache
        .get("hotKeyTop")
        .expect("cache schema declares hotKeyTop");
    assert_eq!(hot["type"], "integer", "the knob is an integer");
    assert_eq!(hot["minimum"], 0, "0 is the legal floor (the bare pair)");
}

/// The typed struct carries the key under the camelCase spelling the
/// schema documents (`deny_unknown_fields` would refuse the section
/// otherwise — the parse pins above hold the same contract).
#[test]
fn cache_config_deserializes_camel_case_hot_key_top() {
    let raw = r#"{ "handler": { "type": "memory" }, "hotKeyTop": 25 }"#;
    let parsed: OxideConfig = serde_json::from_str(&format!(
        r#"{{ "framework": {{ "adapter": "react" }}, "cache": {raw} }}"#
    ))
    .expect("the camelCase spelling parses");
    assert_eq!(parsed.cache.hot_key_top, Some(25));
}
