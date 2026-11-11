//! TDD suite for the V2-F13 config surface: the `prefetch` section —
//! `prefetch.learned` (the master switch, off by default) and
//! `prefetch.minProbability` (the client's prefetch threshold, spec default
//! 0.35). Typed keys, schema declarations (the E11 DOC-001 gate requires a
//! description on every leaf) and the load-time validator that names a bad
//! value before the typed parse (`DSP-115`).
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
fn prefetch_defaults_off_with_the_spec_probability() {
    // The inert-defaults contract the live/stream/brownout sections set:
    // with `learned: false` no NavGraph is constructed, no edge is
    // observed, and the endpoint answers 404 — the request path reads
    // nothing new.
    let cfg = OxideConfig::default();
    assert!(!cfg.prefetch.learned, "learned defaults off");
    assert!(
        (cfg.prefetch.min_probability - 0.35).abs() < f64::EPSILON,
        "minProbability defaults to the spec's 0.35"
    );
}

#[test]
fn prefetch_section_parses_from_camel_case_json() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{
            "framework": { "adapter": "react" },
            "prefetch": { "learned": true, "minProbability": 0.5 }
        }"#,
    )]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
    assert!(loaded.config.prefetch.learned);
    assert!((loaded.config.prefetch.min_probability - 0.5).abs() < 1e-9);
}

#[test]
fn learned_accepts_only_a_real_boolean() {
    // The DSP-114 spelling rule: a quoted "true" (the TS-import shape that
    // leaked into JSON) or a 0/1 is an authoring bug — the load-time
    // validator catches it with DSP-115 before the typed parse.
    for body in [
        r#"{ "framework": { "adapter": "react" }, "prefetch": { "learned": "true" } }"#,
        r#"{ "framework": { "adapter": "react" }, "prefetch": { "learned": 1 } }"#,
    ] {
        let (_t, dir) = fixture(&[("oxide.config.json", body)]);
        let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .expect_err("a non-boolean learned is refused");
        assert!(
            err.to_string().contains("DSP-115"),
            "the diagnostic names DSP-115 (got: {err})"
        );
    }
}

#[test]
fn min_probability_must_be_a_fraction() {
    // Bounds: 0.0 ..= 1.0. Outside the range the feature is nonsense
    // (1.5 prefetches everything it ranks; -0.1 nothing).
    for bad in ["1.5", "-0.1", "2"] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react" }}, "prefetch": {{ "minProbability": {bad} }} }}"#
            ),
        )]);
        let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .expect_err("an out-of-range minProbability is refused");
        assert!(
            err.to_string().contains("DSP-115"),
            "the diagnostic names DSP-115 (got: {err})"
        );
    }
    // A non-numeric spelling is refused too.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "prefetch": { "minProbability": "high" } }"#,
    )]);
    let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
        .expect_err("a non-numeric minProbability is refused");
    assert!(err.to_string().contains("DSP-115"));
}

#[test]
fn min_probability_accepts_the_boundary_values() {
    for ok in ["0", "1", "0.35"] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react" }}, "prefetch": {{ "minProbability": {ok} }} }}"#
            ),
        )]);
        let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
        let expected: f64 = ok.parse().unwrap();
        assert!((loaded.config.prefetch.min_probability - expected).abs() < 1e-9);
    }
}

#[test]
fn unknown_prefetch_keys_are_rejected_by_deny_unknown_fields() {
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "prefetch": { "aggressive": true } }"#,
    )]);
    let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
        .expect_err("an unknown prefetch key is refused");
    assert!(
        err.to_string().contains("aggressive"),
        "the error names the offending key (got: {err})"
    );
}

#[test]
fn prefetch_is_a_known_top_level_key_for_typo_diagnostics() {
    // layer.rs's KNOWN_KEYS drives the typo suggestion ("rename `prefech`
    // to a valid top-level config key"). `prefetch` must be in the list.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" }, "prefech": { "learned": true } }"#,
    )]);
    let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
        .expect_err("an unknown top-level key is refused");
    assert!(
        err.to_string().contains("prefetch"),
        "the fix suggests `prefetch` for the `prefech` typo (got: {err})"
    );
}

#[test]
fn schema_declares_the_prefetch_keys_with_descriptions() {
    // The E11 DOC-001 gate: every leaf key carries a description, so
    // `oxide docs generate` renders the reference page without refusing.
    let schema = oxide_fw_config::config_schema();
    let prefetch = &schema["properties"]["prefetch"];
    assert_eq!(prefetch["type"], "object");
    let props = &prefetch["properties"];
    for key in ["learned", "minProbability"] {
        let leaf = &props[key];
        assert!(
            leaf.get("description").is_some(),
            "prefetch.{key} lacks a description"
        );
    }
    assert_eq!(props["learned"]["type"], "boolean");
    assert_eq!(props["minProbability"]["type"], "number");
}
