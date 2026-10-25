//! §22 / L5 — schema stability for `config_schema()`.

#[test]
fn config_schema_has_the_known_top_level_sections() {
    let schema = oxide_fw_config::config_schema();
    let obj = schema
        .as_object()
        .expect("config_schema() must return a JSON object");
    let props = obj
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("schema must have a 'properties' object");
    for expected in [
        "adapter",
        "build",
        "cache",
        "dev",
        "fonts",
        "framework",
        "i18n",
        "images",
        "modules",
        "og",
        "render",
        "routes",
        "security",
        "server",
        "sw",
        "siteUrl",
    ] {
        assert!(
            props.contains_key(expected),
            "config_schema is missing top-level section {expected:?}. \
             Properties present: {:?}",
            props.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn config_schema_serializes_to_valid_json() {
    let pretty = oxide_fw_config::config_schema_pretty()
        .expect("config_schema_pretty() must not fail on a valid schema");
    // `config_schema_pretty` is pretty-printed with a trailing newline so
    // the output is paste-friendly; trim before asserting on the shape.
    let trimmed = pretty.trim_end();
    assert!(
        trimmed.starts_with('{'),
        "pretty JSON starts with a brace: {trimmed:?}"
    );
    assert!(
        trimmed.ends_with('}'),
        "pretty JSON ends with a brace: {trimmed:?}"
    );
    let _: serde_json::Value =
        serde_json::from_str(trimmed).expect("config_schema_pretty() output must reparse as JSON");
}

#[test]
fn session_ttl_secs_round_trips() {
    // §16.6 / WP-r9: the field exists, defaults to 1800, and parses.
    let default: oxide_fw_config::OxideConfig =
        serde_json::from_value(serde_json::json!({})).expect("default");
    assert_eq!(default.security.session_ttl_secs, 1800);

    let custom: oxide_fw_config::OxideConfig = serde_json::from_value(serde_json::json!({
        "security": { "sessionTtlSecs": 3600 }
    }))
    .expect("custom");
    assert_eq!(custom.security.session_ttl_secs, 3600);
}

#[test]
fn checked_in_schema_matches_the_function() {
    // §19.3: "JSON Schema for config". The schema is consumable by editors
    // only when it exists at a stable path — `schemas/oxide-fw-config.schema.json`
    // at the repo root. This gate fails when a config key is added to
    // `config_schema()` without regenerating the file:
    //
    //   cargo run -p oxide-fw-config --bin oxide-schema -- \
    //     > schemas/oxide-fw-config.schema.json
    //
    // The same pattern `docs-check` uses for generated docs (§v0.2 draft):
    // regeneration is a one-liner, and forgetting is a red build.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is crates/config/../..");
    let checked_in = repo_root.join("schemas/oxide-fw-config.schema.json");
    let on_disk = std::fs::read_to_string(&checked_in).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. Regenerate with:\n  \
             cargo run -p oxide-fw-config --bin oxide-schema -- \
             > schemas/oxide-fw-config.schema.json",
            checked_in.display()
        )
    });
    let generated = oxide_fw_config::config_schema_pretty().expect("schema renders");

    // Semantic comparison. JSON object key order is not meaningful to any
    // consumer (editor, validator, downstream tool) -- only field content is.
    // Byte-for-byte was too strict: serde_json map representation depends on
    // whether `preserve_order` is enabled, which can flip under workspace-wide
    // feature unification (another crate's dev-deps activating it). Comparing
    // parsed Values enforces the actual invariant (same field set + values) and
    // still catches real drift; regeneration remains the fix for drift.
    let on_disk_value: serde_json::Value = serde_json::from_str(&on_disk)
        .unwrap_or_else(|e| panic!("checked-in schema is not valid JSON: {e}"));
    let generated_value: serde_json::Value = serde_json::from_str(&generated)
        .unwrap_or_else(|e| panic!("generated schema is not valid JSON: {e}"));
    assert_eq!(
        on_disk_value, generated_value,
        "schemas/oxide-fw-config.schema.json is stale (semantic diff -- field added, removed, or value changed). Regenerate with:\n  cargo run -p oxide-fw-config --bin oxide-schema -- > schemas/oxide-fw-config.schema.json"
    );
}
