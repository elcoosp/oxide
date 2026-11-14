//! TDD suite for the V2-F02 config surface: the `framework.autoIslands`
//! gate. The spec's §V2-F02 spelling (`experiments.autoIslands: true`)
//! collides with F04's A/B `experiments` array (a different, typed
//! shape), so the gate lands beside the islands registry it governs —
//! `framework.autoIslands`, default `false` (the spec's own default:
//! "a wrong inference silently breaks interactivity, so the default is
//! to defer"), inert at the wiring until the CLI adoption lands (the
//! RFN-002 pattern the `search`/`deploy` keys used).
//!
//! Written BEFORE the implementation; every pin imports public API only.

use std::collections::BTreeMap;

use oxide_fw_config::{OxideConfig, config_schema, load_with_env};

const EMPTY_ENV: BTreeMap<String, String> = BTreeMap::new();

/// A project fixture with one config file; returns (guard, dir).
fn fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }
    let path = dir.path().to_path_buf();
    (dir, path)
}

/// Load one config body.
fn load_body(body: &str) -> oxide_fw_config::LoadedConfig {
    let (_t, dir) = fixture(&[("oxide.config.json", body)]);
    load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap()
}

// ---------------------------------------------------------------- shape

#[test]
fn auto_islands_defaults_false() {
    // The spec's own default: defer unless the author opts in.
    let cfg = OxideConfig::default();
    assert!(!cfg.framework.auto_islands);
}

#[test]
fn auto_islands_parses_the_camel_case_key() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react", "autoIslands": true }
        }"#,
    );
    assert!(loaded.config.framework.auto_islands);
}

#[test]
fn auto_islands_false_round_trips_explicitly() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react", "autoIslands": false }
        }"#,
    );
    assert!(!loaded.config.framework.auto_islands);
}

// ---------------------------------------------------------------- schema

#[test]
fn the_schema_declares_auto_islands() {
    let schema = config_schema();
    let text = schema.to_string();
    assert!(
        text.contains("autoIslands"),
        "the JSON schema names the key (the E11 DOC-001 gate)"
    );
}

#[test]
fn the_schema_keeps_the_committed_artifact_in_sync() {
    // The committed `schemas/oxide-fw-config.schema.json` must carry the
    // key too — a schema edit without a regeneration is a drift the
    // docs-check gate exists to catch.
    let committed = include_str!("../../../schemas/oxide-fw-config.schema.json");
    assert!(
        committed.contains("autoIslands"),
        "the committed schema declares autoIslands"
    );
}
