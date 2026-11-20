//! Integration tests for the V2-F59 `cache.planeMount` config surface
//! (§11.2 / the F57 mount's boot wire joining the layered resolver
//! family — the F46/F48 boolean shape): the key parses through the typed
//! config, the default stays `None` (the mount answers only where the
//! boot opts in — the F40 doctrine), both spellings are legal (`true`
//! mounts the shared plane over the boot's own tier, `false` pins it
//! off), and the JSON schema (`config_schema()`) declares the knob so
//! editors can complete it.
//!
//! Written FIRST (red): failed on the unresolved `plane_mount` field
//! before `CacheConfig::plane_mount` and the schema property existed.

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

/// The operator's opt-in lands: `cache.planeMount: true` parses through
/// the typed config and rides the boot wire (the shared plane answers
/// its families on the site's own socket).
#[test]
fn cache_plane_mount_parses_from_the_config() {
    let loaded = load_config(&config_json(r#"{ "planeMount": true }"#))
        .expect("a cache section with planeMount loads");
    assert_eq!(
        loaded.config.cache.plane_mount,
        Some(true),
        "the project's committed mount opt-in"
    );
}

/// A committed `false` is a real choice too: the baseline can pin the
/// mount off against an env spelling above it (the F46/F48 posture —
/// both boolean spellings are legal).
#[test]
fn cache_plane_mount_false_is_a_legal_choice() {
    let loaded =
        load_config(&config_json(r#"{ "planeMount": false }"#)).expect("planeMount false loads");
    assert_eq!(
        loaded.config.cache.plane_mount,
        Some(false),
        "the project's committed-off shape"
    );
}

/// Without the key the field stays `None` — the default-off shape
/// answers and the boot is byte-identical (the F40 doctrine: the plane
/// answers only where a host opts in).
#[test]
fn cache_plane_mount_defaults_to_none() {
    let loaded = load_config(&config_json("{}")).expect("an empty cache section loads");
    assert_eq!(
        loaded.config.cache.plane_mount, None,
        "the key absent keeps the mount off"
    );
}

/// The JSON schema declares the knob so editors can complete it (the
/// E11 contract: a config key without a schema description fails
/// docs-check).
#[test]
fn cache_schema_declares_plane_mount() {
    let schema = config_schema();
    let cache = &schema["properties"]["cache"]["properties"];
    let mount = cache
        .get("planeMount")
        .expect("cache schema declares planeMount");
    assert_eq!(mount["type"], "boolean", "the knob is the boolean family");
    assert_eq!(mount["default"], false, "the default-off shape");
}

/// The typed struct carries the key under the camelCase spelling the
/// schema documents (`deny_unknown_fields` would refuse the section
/// otherwise — the parse pins above hold the same contract).
#[test]
fn cache_config_deserializes_camel_case_plane_mount() {
    let body = r#"{
        "framework": { "adapter": "react" },
        "cache": { "planeMount": true }
    }"#;
    let cfg: OxideConfig = serde_json::from_str(body).expect("camelCase key deserializes");
    assert_eq!(
        cfg.cache.plane_mount,
        Some(true),
        "the schema's camelCase spelling rides the typed struct"
    );
}
