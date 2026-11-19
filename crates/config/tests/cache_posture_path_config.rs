//! Integration tests for the V2-F50 `cache.postureProbePath` config
//! surface (§20 / the F49 probe's surface location joining the layered
//! resolver family — the F42/F43/F48 shape over a String value): the
//! key parses through the typed [`OxideConfig`], the default stays
//! `None` (the recorded `/-/health` path answers at the wire when the
//! config never speaks), both spellings are legal (a `/`-led path
//! relocates the probe, the empty string pins the surface off), and the
//! JSON schema (`config_schema()`) declares the knob so editors can
//! complete it.
//!
//! Written FIRST (red): failed on the unresolved `posture_probe_path`
//! field before `CacheConfig::posture_probe_path` and the schema
//! property existed.

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

/// The operator's relocation lands: `cache.postureProbePath:
/// "/-/cache-posture"` parses through the typed config and rides the
/// boot wire.
#[test]
fn cache_posture_path_parses_from_the_config() {
    let loaded = load_config(&config_json(
        r#"{ "postureProbePath": "/-/cache-posture" }"#,
    ))
    .expect("a cache section with postureProbePath loads");
    assert_eq!(
        loaded.config.cache.posture_probe_path.as_deref(),
        Some("/-/cache-posture"),
        "the project's committed probe path"
    );
}

/// A committed empty string is a real choice too: the baseline can pin
/// the surface OFF (the ready-path shape — the knob is not a one-way
/// opt-in).
#[test]
fn cache_posture_path_empty_is_a_legal_choice() {
    let loaded = load_config(&config_json(r#"{ "postureProbePath": "" }"#))
        .expect("postureProbePath empty loads");
    assert_eq!(
        loaded.config.cache.posture_probe_path.as_deref(),
        Some(""),
        "the project's committed-off shape"
    );
}

/// Without the key the field stays `None` — the recorded default path
/// answers at the wire and the boot shape is byte-identical (the F40
/// doctrine).
#[test]
fn cache_posture_path_defaults_to_none() {
    let loaded = load_config(&config_json("{}")).expect("an empty cache section loads");
    assert_eq!(
        loaded.config.cache.posture_probe_path, None,
        "the key absent keeps the recorded default path"
    );
}

/// The schema declares the knob: type string, default the recorded
/// path — so editors complete `postureProbePath` and the docs page
/// renders it.
#[test]
fn cache_schema_declares_posture_path() {
    let schema = config_schema();
    let cache = &schema["properties"]["cache"]["properties"];
    let path = cache
        .get("postureProbePath")
        .expect("cache schema declares postureProbePath");
    assert_eq!(path["type"], "string", "the knob is a string path");
    assert_eq!(path["default"], "/-/health", "the recorded default path");
}

/// The typed struct carries the key under the camelCase spelling the
/// schema documents (`deny_unknown_fields` would refuse the section
/// otherwise — the parse pins above hold the same contract).
#[test]
fn cache_config_deserializes_camel_case_posture_path() {
    let body = r#"{
        "framework": { "adapter": "react" },
        "cache": { "postureProbePath": "/-/cache-posture" }
    }"#;
    let cfg: OxideConfig = serde_json::from_str(body).expect("camelCase key deserializes");
    assert_eq!(
        cfg.cache.posture_probe_path.as_deref(),
        Some("/-/cache-posture"),
        "the schema's camelCase spelling rides the typed struct"
    );
}
