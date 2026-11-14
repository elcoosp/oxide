//! TDD suite for the V2-F10 config surface: the `deploy` section —
//! `deploy.skewWindow` (the spec's duration string, default `'15m'`) and
//! `deploy.generations` (the hard-linked generation count, default 3).
//! Typed keys, schema declarations (the E11 DOC-001 gate) and the
//! load-time validator that names a bad duration spelling or a
//! physically useless bound before the typed parse (`CFG-DEPLOY`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The section is inert at the wiring until the F10 CLI adoption lands
//! (booting the serving pools from `dist/` + F11's `releases/` layout,
//! the promote-on-reload, the 30 s expiry task, the F12 brownout trigger)
//! — the same RFN-002 pattern the `search`/`record` keys used. The
//! engine-side default (`DEFAULT_SKEW_WINDOW`, `crates/adapters`) stays
//! the plumbing default; the typed key is its config face.

use oxide_fw_config::{OxideConfig, config_schema, load_with_env, parse_duration_secs};
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
fn deploy_defaults_match_the_spec_code_block() {
    // Appendix V2.L: `deploy?: { skewWindow?: string; generations?: number
    // }` (defaults '15m', 3).
    let cfg = OxideConfig::default();
    assert_eq!(cfg.deploy.skew_window, "15m");
    assert_eq!(cfg.deploy.generations, 3);
}

#[test]
fn deploy_section_parses_camel_case() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "skewWindow": "2m", "generations": 5 }
        }"#,
    )
    .unwrap();
    assert_eq!(loaded.config.deploy.skew_window, "2m");
    assert_eq!(loaded.config.deploy.generations, 5);
}

// ---------------------------------------------------- the duration grammar

#[test]
fn parse_duration_reads_the_grammar() {
    // `<n>s|m|h|d`, plain integers = seconds — the §14 shape the size
    // literals already use, extended to time.
    assert_eq!(parse_duration_secs("30").unwrap(), 30);
    assert_eq!(parse_duration_secs("30s").unwrap(), 30);
    assert_eq!(parse_duration_secs("15m").unwrap(), 15 * 60);
    assert_eq!(parse_duration_secs("1h").unwrap(), 3600);
    assert_eq!(parse_duration_secs("2d").unwrap(), 2 * 86400);
}

#[test]
fn parse_duration_refuses_the_bad_spellings() {
    for bad in ["15x", "m", "1.5h", "15 m", "-1m", "999999999999999999999d"] {
        assert!(
            parse_duration_secs(bad).is_err(),
            "`{bad}` is not a duration literal"
        );
    }
}

// ---------------------------------------------------------- CFG-DEPLOY

#[test]
fn deploy_bad_duration_is_cfg_deploy() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "skewWindow": "quarter hour" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
    assert!(
        err.0.message.contains("<n>s|m|h|d") || err.0.message.contains("`15m`"),
        "the message names the grammar (got: {})",
        err.0.message
    );
}

#[test]
fn deploy_zero_window_is_cfg_deploy() {
    // A zero window expires the previous generation instantly — the
    // physically-useless-bound shape DSP-112 names for the stream ttl.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "skewWindow": "0s" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
}

#[test]
fn deploy_generations_below_one_is_cfg_deploy() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "generations": 0 }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
}

#[test]
fn deploy_unknown_object_key_is_cfg_deploy() {
    // deny_unknown_fields is the section contract: a typo'd knob cannot
    // silently disable skew protection — the raw pass names the two
    // valid knobs.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "skewWindows": "15m" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
    assert!(
        err.0.message.contains("skewWindow") && err.0.message.contains("generations"),
        "the message names the valid knobs (got: {})",
        err.0.message
    );
}

#[test]
fn deploy_wrong_typed_knobs_are_cfg_deploy() {
    // A quoted "15m" (the TS-import leak) and a quoted 3 are authoring
    // bugs, not values.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "skewWindow": 15 }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");

    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": { "generations": "3" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
}

#[test]
fn deploy_non_object_shape_is_cfg_deploy() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deploy": "15m"
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-DEPLOY");
}

// ------------------------------------------------------- typo surface

#[test]
fn deploy_is_a_known_top_level_key_for_typo_diagnostics() {
    // layer.rs's KNOWN_KEYS drives the typo suggestion ("rename `deployy`
    // to a valid top-level config key") — the key must be in the list or
    // a typo'd section is rejected as unknown instead of suggested.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "deployy": { "generations": 3 }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-UNKNOWN-KEY");
    assert!(
        err.0.message.contains("`deploy`"),
        "the suggestion names `deploy` (got: {})",
        err.0.message
    );
}

// ------------------------------------------------------------- schema

#[test]
fn schema_declares_the_deploy_section() {
    // The E11 DOC-001 gate: every key has a schema entry with a
    // description, so editor tooling completes and validates the shape.
    let schema = config_schema();
    let deploy = &schema["properties"]["deploy"];
    assert!(
        deploy["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the section is described (DOC-001)"
    );
    let props = &deploy["properties"];
    assert!(
        props["skewWindow"]["default"] == "15m"
            && props["skewWindow"]["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
        "skewWindow is declared with its spec default (got: {})",
        props["skewWindow"]
    );
    assert!(
        props["generations"]["default"] == 3
            && props["generations"]["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
        "generations is declared with its spec default (got: {})",
        props["generations"]
    );
}
