//! §22 / L5 — schema stability for `config_schema()`.

#[test]
fn config_schema_has_the_known_top_level_sections() {
    let schema = oxide_config::config_schema();
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
    let pretty = oxide_config::config_schema_pretty()
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
    let default: oxide_config::OxideConfig =
        serde_json::from_value(serde_json::json!({})).expect("default");
    assert_eq!(default.security.session_ttl_secs, 1800);

    let custom: oxide_config::OxideConfig =
        serde_json::from_value(serde_json::json!({
            "security": { "sessionTtlSecs": 3600 }
        }))
        .expect("custom");
    assert_eq!(custom.security.session_ttl_secs, 3600);
}

