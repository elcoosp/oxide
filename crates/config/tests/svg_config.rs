//! TDD suite for the V2-C07 config surface: `svg.optimize` — the SVG
//! optimizer knob. Typed key (`SvgConfig`), schema declaration (the E11
//! DOC-001 gate requires a description on every leaf) and the default-off
//! passthrough contract: a build that did not opt in never mutates a
//! document.
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
fn svg_optimize_defaults_to_off() {
    // §V2-C07: the knob is opt-in — an untouched config keeps the
    // byte-identical passthrough (the assets default is `IdentitySvg`).
    let cfg = OxideConfig::default();
    assert!(!cfg.svg.optimize, "svg.optimize defaults to false");
}

#[test]
fn svg_optimize_parses_from_camel_case_json() {
    for (raw, want) in [("true", true), ("false", false)] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react" }}, "svg": {{ "optimize": {raw} }} }}"#
            ),
        )]);
        let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .unwrap_or_else(|e| panic!("svg.optimize {raw} loads: {e}"));
        assert_eq!(loaded.config.svg.optimize, want);
    }
}

#[test]
fn svg_section_refuses_unknown_keys() {
    // The config's deny_unknown_fields contract: a typo'd leaf is a load
    // error, not a silently ignored key.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "svg": { "optimise": true } }"#,
    )]);
    let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
        .expect_err("an unknown svg key is refused");
    let text = err.to_string();
    assert!(
        text.contains("optimise") || text.contains("unknown"),
        "the refusal names the bad key (got: {text})"
    );
}

#[test]
fn schema_declares_svg_optimize_with_a_description() {
    // The E11 DOC-001 gate: every leaf key carries a description.
    let schema = oxide_fw_config::config_schema();
    let leaf = &schema["properties"]["svg"]["properties"]["optimize"];
    assert!(
        leaf.get("description").is_some(),
        "svg.optimize lacks a description"
    );
    assert_eq!(leaf["type"], "boolean");
    assert_eq!(leaf["default"], serde_json::json!(false));
}
