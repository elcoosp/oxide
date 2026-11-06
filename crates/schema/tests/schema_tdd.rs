//! `oxide-fw-schema` integration tests — the `oxide.config.ts` schema contract.
//!
//! The schema is the contract between a user's `oxide.config.ts` and their
//! editor (`$schema` → real-time validation) AND the machine contract printed
//! by `oxide schema`. The committed `schemas/oxide-fw-config.schema.json` is
//! what users download; the config crate's `config_schema_pretty` is the one
//! source of truth. These tests pin the triangle: source-of-truth function ↔
//! committed artifact ↔ valid JSON Schema shape, plus the error-code paths
//! (SCHEMA-001/002) the CLI surfaces.

use oxide_fw_schema::{schema_pretty, schema_url, schema_value, CODE_CONFIG_FAILURE, CODE_NOT_JSON, SCHEMA_URL};

#[test]
fn schema_value_is_a_valid_json_schema_object() {
    let v = schema_value().expect("schema_value");
    assert!(v.is_object());
    // Minimal JSON Schema shape an editor relies on:
    assert!(v.get("$schema").is_some(), "must declare $schema for editor binding");
    assert!(v.get("properties").is_some(), "config keys validate via properties");
    assert!(v.get("type").is_some(), "root type must be pinned");
}

#[test]
fn schema_pretty_is_stable_across_calls() {
    let a = schema_pretty().expect("schema_pretty a");
    let b = schema_pretty().expect("schema_pretty b");
    assert_eq!(a, b, "the emitted schema must be deterministic (editors cache it)");
}

#[test]
fn schema_delegates_to_the_config_crate_source_of_truth() {
    // Exactly one source of truth: oxide-fw-config. A fork between the two
    // would let `oxide schema` and editor validation disagree. The delegation
    // wraps config's error into SCHEMA-001, so on success the BYTES match.
    let from_config = oxide_fw_config::config_schema_pretty().expect("config schema");
    let via_schema = schema_pretty().expect("schema_pretty");
    assert_eq!(via_schema, from_config, "schema crate must not fork the config schema");
}

#[test]
fn committed_schema_artifact_matches_the_generated_schema() {
    let committed = include_str!("../../../schemas/oxide-fw-config.schema.json");
    let generated = schema_pretty().expect("schema_pretty");
    assert_eq!(
        committed.trim_end(),
        generated.trim_end(),
        "schemas/oxide-fw-config.schema.json is stale — regenerate with `oxide schema`"
    );
}

#[test]
fn schema_url_is_the_canonical_https_url() {
    assert_eq!(schema_url(), SCHEMA_URL);
    assert!(
        SCHEMA_URL.starts_with("https://"),
        "editors fetch over TLS; a non-https URL would fail strict editors"
    );
}

#[test]
fn error_codes_are_part_of_the_public_contract() {
    // `oxide schema` failures surface these codes to users and scripts; a
    // rename would be a breaking change like any CLI output change.
    assert_eq!(CODE_CONFIG_FAILURE, "SCHEMA-001");
    assert_eq!(CODE_NOT_JSON, "SCHEMA-002");
}
