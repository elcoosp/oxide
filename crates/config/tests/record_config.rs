//! TDD suite for the V2-F15 config surface: the `record` key — the spec
//! Appendix V2.L shape (`false | 'errors' | 'all' | { sample, onError }`)
//! typed, schema'd (the E11 DOC-001 gate requires a description on every
//! leaf) and validated at load before the typed parse (`CFG-REC` names a
//! bad spelling, an out-of-range sample or a wrong-typed knob instead of
//! a bare serde type error).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The key is inert at the wiring until the CLI's record/replay adoption
//! lands (the deferred A01-phase-3 half): the substrate's
//! `RecordPolicy` is the decision, the typed key is its config face —
//! the same RFN-002 pattern the `search` key used (enforcement named in
//! the registry, live from day one; the consumer arrives later).

use oxide_fw_config::{OxideConfig, config_schema, load_with_env};
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

/// Load one config body; the error is asserted through `err.0.code`.
fn load_body(
    body: &str,
) -> Result<oxide_fw_config::LoadedConfig, oxide_fw_diagnostics::OxideError> {
    let (_t, dir) = fixture(&[("oxide.config.json", body)]);
    load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
}

// ---------------------------------------------------------------- shape

#[test]
fn record_defaults_to_off() {
    // The inert-defaults contract every optional section sets: with the
    // key absent nothing records — recording is opt-in (the R05 posture:
    // records are dev-only unless sampled with redaction forced on).
    let cfg = OxideConfig::default();
    assert_eq!(cfg.record, oxide_fw_config::RecordConfig::Off(false));
}

#[test]
fn record_false_explicitly_matches_the_default() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": false
        }"#,
    )
    .unwrap();
    assert_eq!(
        loaded.config.record,
        oxide_fw_config::RecordConfig::Off(false)
    );
}

#[test]
fn record_word_spellings_parse() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": "all"
        }"#,
    )
    .unwrap();
    assert_eq!(loaded.config.record, oxide_fw_config::RecordConfig::All);

    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": "errors"
        }"#,
    )
    .unwrap();
    assert_eq!(loaded.config.record, oxide_fw_config::RecordConfig::Errors);
}

#[test]
fn record_sampled_object_parses_with_defaults() {
    // The spec's prod example is `{ sample: 0.001, onError: true }`; the
    // object's knobs default to that shape so `{}` is the spec default.
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": { "sample": 0.5 }
        }"#,
    )
    .unwrap();
    assert_eq!(
        loaded.config.record,
        oxide_fw_config::RecordConfig::Sampled {
            sample: 0.5,
            on_error: true
        }
    );

    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": {}
        }"#,
    )
    .unwrap();
    assert_eq!(
        loaded.config.record,
        oxide_fw_config::RecordConfig::Sampled {
            sample: 0.001,
            on_error: true
        }
    );
}

// ------------------------------------------------------------ CFG-REC

#[test]
fn record_true_is_refused_with_cfg_rec() {
    // The spec shape is `false | 'errors' | 'all' | { ... }` — `true` is
    // not a mode; the diagnostic names the boolean spelling that exists.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": true
        }"#,
    )
    .unwrap_err();
    assert_eq!(
        err.0.code, "CFG-REC",
        "true is an authoring bug (got: {})",
        err.0.message
    );
    assert!(
        err.0.message.contains("`all`") && err.0.message.contains("`errors`"),
        "the message names the two word modes (got: {})",
        err.0.message
    );
}

#[test]
fn record_bad_word_names_the_two_modes() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": "every"
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-REC");
    assert!(
        err.0.message.contains("`all`") && err.0.message.contains("`errors`"),
        "the message names the valid spellings (got: {})",
        err.0.message
    );
}

#[test]
fn record_sample_out_of_range_is_cfg_rec() {
    for bad in ["1.5", "-0.1", "2"] {
        let err = load_body(&format!(
            r#"{{
                "framework": {{ "adapter": "react" }},
                "record": {{ "sample": {bad} }}
            }}"#
        ))
        .unwrap_err();
        assert_eq!(err.0.code, "CFG-REC", "sample {bad} is out of range");
        assert!(
            err.0.message.contains('0') && err.0.message.contains('1'),
            "the message names the range (got: {})",
            err.0.message
        );
    }
}

#[test]
fn record_sample_must_be_a_real_number() {
    // A quoted "0.5" (the TS-import leak the other validators name) or a
    // boolean roll is an authoring bug, not a sample rate.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": { "sample": "0.5" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-REC");

    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": { "sample": true }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-REC");
}

#[test]
fn record_on_error_must_be_a_real_boolean() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": { "onError": "yes" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(
        err.0.code, "CFG-REC",
        "a quoted onError is an authoring bug"
    );
}

#[test]
fn record_unknown_object_key_is_cfg_rec() {
    // deny_unknown_fields is the section contract (the search/stream
    // pattern): a typo'd knob cannot silently disable the feature — the
    // raw pass names the two valid knobs.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "record": { "sample": 0.1, "rate": 1 }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-REC");
    assert!(
        err.0.message.contains("sample") && err.0.message.contains("onError"),
        "the message names the valid knobs (got: {})",
        err.0.message
    );
}

#[test]
fn record_other_shapes_are_cfg_rec() {
    for bad in ["42", "[\"all\"]", "null"] {
        let err = load_body(&format!(
            r#"{{
                "framework": {{ "adapter": "react" }},
                "record": {bad}
            }}"#
        ))
        .unwrap_err();
        assert_eq!(err.0.code, "CFG-REC", "the shape {bad} is not a record key");
    }
}

// ------------------------------------------------------- typo surface

#[test]
fn record_is_a_known_top_level_key_for_typo_diagnostics() {
    // layer.rs's KNOWN_KEYS drives the typo suggestion ("rename `recored`
    // to a valid top-level config key") — the key must be in the list or
    // a typo'd section is rejected as unknown instead of suggested.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "recored": "all"
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-UNKNOWN-KEY");
    assert!(
        err.0.message.contains("`record`"),
        "the suggestion names `record` (got: {})",
        err.0.message
    );
}

// ------------------------------------------------------------- schema

#[test]
fn schema_declares_the_record_section() {
    // The E11 DOC-001 gate: every key has a schema entry with a
    // description, so editor tooling completes and validates the shape.
    let schema = config_schema();
    let record = &schema["properties"]["record"];
    assert!(
        record["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the section is described (DOC-001)"
    );
    assert!(
        record.get("anyOf").is_some(),
        "the union shape is declared (got: {record})"
    );
    let any = record["anyOf"].as_array().unwrap();
    assert!(
        any.iter()
            .any(|arm| arm.get("type").and_then(|t| t.as_str()) == Some("boolean")),
        "the boolean arm is declared"
    );
    assert!(
        any.iter().any(|arm| arm["enum"]
            .as_array()
            .is_some_and(|e| e.iter().any(|v| v.as_str() == Some("all") && e.len() == 2))),
        "the word arm declares exactly `all` / `errors`"
    );
    assert!(
        any.iter()
            .any(|arm| arm["properties"].get("sample").is_some()
                && arm["properties"].get("onError").is_some()),
        "the sampled object arm declares both knobs"
    );
}
