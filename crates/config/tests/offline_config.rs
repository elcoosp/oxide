//! TDD suite for the V2-F14 config surface: the `offline` section —
//! `offline.enabled`, `offline.precache` (`'routes' | 'all'`) and
//! `offline.outbox`. Typed keys, schema declarations (the E11 DOC-001
//! gate) and the load-time validator that names a bad spelling or a
//! wrong-typed knob before the typed parse (`CFG-OFFLINE`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The section is inert at the wiring until the F14 CLI adoption lands
//! (the PWA worker's precache emitting from the manifest, the client
//! outbox boot) — the same RFN-002 pattern the `search`/`record`/
//! `deploy` keys used. The server-side stage the outbox retries hit is
//! `oxide_fw_dispatch::idem::stage_idempotency` (the same milestone).

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
fn offline_defaults_match_the_spec_shape() {
    // Appendix V2.L: `offline?: { enabled?: boolean; precache?: 'routes'
    // | 'all'; outbox?: boolean }` — off by default, the routes-breadth
    // precache, the outbox on (the spec's example shape).
    let cfg = OxideConfig::default();
    assert!(!cfg.offline.enabled);
    assert!(matches!(
        cfg.offline.precache,
        oxide_fw_config::OfflinePrecache::Routes
    ));
    assert!(cfg.offline.outbox);
}

#[test]
fn offline_section_parses_camel_case() {
    let loaded = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": { "enabled": true, "precache": "all", "outbox": false }
        }"#,
    )
    .unwrap();
    assert!(loaded.config.offline.enabled);
    assert!(matches!(
        loaded.config.offline.precache,
        oxide_fw_config::OfflinePrecache::All
    ));
    assert!(!loaded.config.offline.outbox);
}

// ---------------------------------------------------------- CFG-OFFLINE

#[test]
fn offline_bad_precache_spelling_is_cfg_offline() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": { "enabled": true, "precache": "everything" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-OFFLINE");
    assert!(
        err.0.message.contains("routes") && err.0.message.contains("all"),
        "the message names the valid spellings (got: {})",
        err.0.message
    );
}

#[test]
fn offline_non_boolean_knobs_are_cfg_offline() {
    // A quoted true and a string outbox are authoring bugs, not values.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": { "enabled": "true" }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-OFFLINE");

    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": { "outbox": 1 }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-OFFLINE");
}

#[test]
fn offline_unknown_object_key_is_cfg_offline() {
    // deny_unknown_fields is the section contract: a typo'd knob cannot
    // silently disable the feature — the raw pass names the three
    // valid knobs.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": { "enabled": true, "offlineFirst": true }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-OFFLINE");
    assert!(
        err.0.message.contains("enabled")
            && err.0.message.contains("precache")
            && err.0.message.contains("outbox"),
        "the message names the valid knobs (got: {})",
        err.0.message
    );
}

#[test]
fn offline_non_object_shape_is_cfg_offline() {
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offline": true
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-OFFLINE");
}

// ------------------------------------------------------- typo surface

#[test]
fn offline_is_a_known_top_level_key_for_typo_diagnostics() {
    // layer.rs's KNOWN_KEYS drives the typo suggestion ("rename
    // `offlin` to a valid top-level config key") — the key must be in
    // the list or a typo'd section is rejected as unknown instead of
    // suggested.
    let err = load_body(
        r#"{
            "framework": { "adapter": "react" },
            "offlin": { "enabled": true }
        }"#,
    )
    .unwrap_err();
    assert_eq!(err.0.code, "CFG-UNKNOWN-KEY");
    assert!(
        err.0.message.contains("`offline`"),
        "the suggestion names `offline` (got: {})",
        err.0.message
    );
}

// ------------------------------------------------------------- schema

#[test]
fn schema_declares_the_offline_section() {
    // The E11 DOC-001 gate: every key has a schema entry with a
    // description, so editor tooling completes and validates the shape.
    let schema = config_schema();
    let offline = &schema["properties"]["offline"];
    assert!(
        offline["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "the section is described (DOC-001)"
    );
    let props = &offline["properties"];
    assert!(
        props["enabled"]["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "enabled is described"
    );
    assert!(
        props["precache"]["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "precache is described"
    );
    assert!(
        props["outbox"]["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty()),
        "outbox is described"
    );
}
