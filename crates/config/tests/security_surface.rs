//! TDD suite for the V2-E04 config surface (the Rust half): the
//! `security.sri` flag, the `security.trustedTypes` mode and the
//! `csp: "report-only"` spelling — typed keys, schema declarations and the
//! load-time validator that names a bad spelling before the typed parse.
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
fn sri_defaults_on_and_trusted_types_defaults_off() {
    // V2-E04: SRI is "default on in production builds" — the typed default
    // is `true`; the dev server's `/@boot/` virtual module never carries a
    // manifest digest, so dev is unaffected without a dev/prod key split.
    // Trusted Types is `off` until a project opts in (it changes the CSP
    // bytes, so the inert-defaults contract applies).
    let cfg = OxideConfig::default();
    assert!(cfg.security.sri, "sri defaults on");
    assert_eq!(
        cfg.security.trusted_types, "off",
        "trusted types default off"
    );
}

#[test]
fn report_only_csp_mode_and_the_new_keys_parse_from_camel_case_json() {
    // The `csp` mode gains the `report-only` spelling and the two new keys
    // ride the standard camelCase extraction (the static extractor never
    // runs JavaScript — §2.6).
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{
            "framework": { "adapter": "react" },
            "security": {
                "csp": "report-only",
                "sri": true,
                "trustedTypes": "enforce"
            }
        }"#,
    )]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert_eq!(loaded.config.security.csp, "report-only");
    assert!(loaded.config.security.sri);
    assert_eq!(loaded.config.security.trusted_types, "enforce");
}

#[test]
fn sri_can_be_turned_off_explicitly() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "security": { "sri": false } }"#,
    )]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert!(!loaded.config.security.sri);
}

#[test]
fn bad_trusted_types_spelling_gets_the_cfg_sec_diagnostic() {
    // The load-time validator (the DSP-113 pattern for knob spellings): a
    // bad `trustedTypes` value is an authoring bug and gets a named
    // `CFG-SEC` diagnostic with a fix hint, not a bare serde type error at
    // the typed boundary.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "security": { "trustedTypes": "strict" } }"#,
    )]);
    let e = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    let msg = &e.0.message;
    assert_eq!(e.0.code, "CFG-SEC", "got: {msg}");
    assert!(msg.contains("trustedTypes"), "{msg}");
    assert!(
        e.0.fix.is_some(),
        "the diagnostic carries the valid spellings as the fix"
    );
}

#[test]
fn bad_csp_spelling_gets_the_cfg_sec_diagnostic() {
    // `report-only` joined the enum; an unknown spelling (e.g. `reportonly`)
    // names the four valid modes.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "security": { "csp": "reportonly" } }"#,
    )]);
    let e = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap_err();
    assert_eq!(e.0.code, "CFG-SEC", "got: {}", e.0.message);
    assert!(e.0.message.contains("report-only"), "{}", e.0.message);
}

#[test]
fn schema_declares_the_e04_security_keys() {
    // The E11 acceptance: a config key without a schema description fails
    // docs-check. Both new keys must appear in the generated schema with a
    // description, and `csp` must enumerate `report-only`.
    let v = oxide_fw_config::config_schema();
    let sec = &v["properties"]["security"]["properties"];
    for k in ["sri", "trustedTypes"] {
        let key = sec
            .get(k)
            .unwrap_or_else(|| panic!("security.{k} missing from schema"));
        let desc = key
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        assert!(!desc.is_empty(), "security.{k} carries a description");
    }
    let csp_enum = sec["csp"]["enum"].as_array().expect("csp enum");
    assert!(
        csp_enum.iter().any(|e| e.as_str() == Some("report-only")),
        "csp enum must contain report-only, got {csp_enum:?}"
    );
}
