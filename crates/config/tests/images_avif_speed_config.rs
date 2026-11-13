//! TDD suite for the V2-C09 config surface: `images.avifSpeed` — the AVIF
//! encoder speed knob (ravif's `with_speed`, 1..=10, spec default 6).
//! Typed key, schema declaration (the E11 DOC-001 gate requires a
//! description on every leaf) and the load-time validator that names the
//! range before the typed parse (`CFG-AVIF`).
//!
//! Written BEFORE the implementation; every pin imports public API only.

use oxide_fw_config::{OxideConfig, load_with_env};
use std::collections::BTreeMap;

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

#[test]
fn avif_speed_defaults_to_the_spec_middle_rate() {
    // §V2-C09: the documented default is 6 — ravif's middle rate; the
    // knob is optional so an untouched config keeps the pre-C09 shape.
    let cfg = OxideConfig::default();
    assert_eq!(cfg.images.avif_speed, 6, "avifSpeed defaults to 6");
}

#[test]
fn avif_speed_parses_from_camel_case_json() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "images": { "avifSpeed": 4 } }"#,
    )]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert_eq!(loaded.config.images.avif_speed, 4);
}

#[test]
fn avif_speed_must_be_whole_in_the_ravif_range() {
    // ravif's `with_speed` domain is 1..=10; 0 is nonsense, 11 is out of
    // range, a float is an authoring bug. CFG-AVIF names the range.
    for bad in ["0", "11", "4.5", "\"6\"", "-1"] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react" }}, "images": {{ "avifSpeed": {bad} }} }}"#
            ),
        )]);
        let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .expect_err("an out-of-range avifSpeed is refused");
        let text = err.to_string();
        assert!(
            text.contains("CFG-AVIF"),
            "the diagnostic names CFG-AVIF (got: {text})"
        );
        assert!(
            text.contains("1..=10"),
            "the diagnostic names the range (got: {text})"
        );
    }
    for good in ["1", "10", "6"] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react" }}, "images": {{ "avifSpeed": {good} }} }}"#
            ),
        )]);
        let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .expect("an in-range avifSpeed loads");
        assert_eq!(loaded.config.images.avif_speed, good.parse::<u8>().unwrap());
    }
}

#[test]
fn schema_declares_avif_speed_with_a_description() {
    // The E11 DOC-001 gate: every leaf key carries a description.
    let schema = oxide_fw_config::config_schema();
    let leaf = &schema["properties"]["images"]["properties"]["avifSpeed"];
    assert!(
        leaf.get("description").is_some(),
        "images.avifSpeed lacks a description"
    );
    assert_eq!(leaf["type"], "integer");
    assert_eq!(leaf["default"], serde_json::json!(6));
    assert_eq!(leaf["minimum"], serde_json::json!(1));
    assert_eq!(leaf["maximum"], serde_json::json!(10));
}
