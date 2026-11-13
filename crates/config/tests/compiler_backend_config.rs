//! TDD suite for the V2-C01 config surface: `framework.compilerBackend` —
//! the `auto` (default) | `native` | `node` switch the precompile
//! integration consults (spec §V2-C01, D26). Typed key, schema declaration
//! (the E11 DOC-001 gate requires a description on every leaf), the
//! `OXIDE_COMPILER_BACKEND` env override and the load-time validator that
//! names the three valid spellings before the typed parse (`CFG-BACKEND`).
//!
//! Written BEFORE the implementation; every pin imports public API only.

use oxide_fw_config::{CompilerBackend, OxideConfig, load_with_env};
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
fn compiler_backend_defaults_to_auto() {
    // D26: `auto` picks a native compiler when it is compiled in and
    // healthy, and silently uses the Node pool otherwise — the default
    // must not hard-require anything.
    let cfg = OxideConfig::default();
    assert_eq!(cfg.framework.compiler_backend, CompilerBackend::Auto);
}

#[test]
fn compiler_backend_parses_the_three_spellings() {
    for (spelling, expected) in [
        ("auto", CompilerBackend::Auto),
        ("native", CompilerBackend::Native),
        ("node", CompilerBackend::Node),
    ] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(
                r#"{{ "framework": {{ "adapter": "react", "compilerBackend": "{spelling}" }} }}"#
            ),
        )]);
        let loaded = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV).unwrap();
        assert_eq!(
            loaded.config.framework.compiler_backend, expected,
            "`{spelling}` parses"
        );
    }
}

#[test]
fn env_override_wins_over_the_file() {
    // The spec names the override: `OXIDE_COMPILER_BACKEND`. The env
    // layer sits above the config file and below CLI overrides.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react", "compilerBackend": "auto" } }"#,
    )]);
    let env = BTreeMap::from([("OXIDE_COMPILER_BACKEND".to_string(), "node".to_string())]);
    let loaded = load_with_env(&dir, "production", BTreeMap::new(), &env).unwrap();
    assert_eq!(
        loaded.config.framework.compiler_backend,
        CompilerBackend::Node
    );

    // And the bare env (no file, adapter present so the load passes)
    // applies too.
    let (_t2, dir2) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" } }"#,
    )]);
    let loaded2 = load_with_env(&dir2, "production", BTreeMap::new(), &env).unwrap();
    assert_eq!(
        loaded2.config.framework.compiler_backend,
        CompilerBackend::Node
    );
}

#[test]
fn invalid_value_gives_the_typo_diagnostic_with_the_three_choices() {
    // The CFG-BACKEND validator runs before the typed parse so a bad
    // spelling names the three valid ones instead of a raw serde error.
    for bad in ["nativ", "NATIVE", "rust", "2"] {
        let (_t, dir) = fixture(&[(
            "oxide.config.json",
            &format!(r#"{{ "framework": {{ "adapter": "react", "compilerBackend": "{bad}" }} }}"#),
        )]);
        let err = load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
            .expect_err("an unknown compilerBackend spelling is refused");
        let text = err.to_string();
        assert!(
            text.contains("CFG-BACKEND"),
            "the diagnostic names CFG-BACKEND (got: {text})"
        );
        assert!(
            text.contains(bad),
            "the diagnostic echoes the bad value (got: {text})"
        );
        for choice in ["auto", "native", "node"] {
            assert!(
                text.contains(choice),
                "the diagnostic names the choice `{choice}` (got: {text})"
            );
        }
    }
    // The env spelling is validated with the same diagnostic (the
    // adapter is present so the CFG-FRAMEWORK gate passes).
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" } }"#,
    )]);
    let env = BTreeMap::from([("OXIDE_COMPILER_BACKEND".to_string(), "wasm".to_string())]);
    let err = load_with_env(&dir, "production", BTreeMap::new(), &env)
        .expect_err("a bad env spelling is refused");
    assert!(err.to_string().contains("CFG-BACKEND"), "got: {err}");
}

#[test]
fn cli_override_wins_over_env() {
    // The recorded layering: file → env → CLI overrides.
    let (_t, dir) = fixture(&[(
        "oxide.config.json",
        r#"{ "framework": { "adapter": "react" } }"#,
    )]);
    let env = BTreeMap::from([("OXIDE_COMPILER_BACKEND".to_string(), "node".to_string())]);
    let cli = BTreeMap::from([(
        "framework.compilerBackend".to_string(),
        serde_json::json!("native"),
    )]);
    let loaded = load_with_env(&dir, "production", cli, &env).unwrap();
    assert_eq!(
        loaded.config.framework.compiler_backend,
        CompilerBackend::Native
    );
}

#[test]
fn schema_declares_compiler_backend_with_a_description() {
    // The E11 DOC-001 gate: every leaf key carries a description, so
    // `oxide docs generate` renders the reference page without refusing.
    let schema = oxide_fw_config::config_schema();
    let leaf = &schema["properties"]["framework"]["properties"]["compilerBackend"];
    assert!(
        leaf.get("description").is_some(),
        "framework.compilerBackend lacks a description"
    );
    assert_eq!(leaf["type"], "string");
    let choices: Vec<&str> = leaf["enum"].as_array().map_or_else(Default::default, |a| {
        a.iter().filter_map(|v| v.as_str()).collect()
    });
    assert_eq!(choices, vec!["auto", "native", "node"]);
}

#[test]
fn compiler_backend_is_honest_in_the_serialized_shape() {
    // Round-trip: the typed default serializes back to the camelCase key
    // with the lowercase spelling the TS side writes.
    let json = serde_json::to_value(OxideConfig::default()).unwrap();
    assert_eq!(
        json["framework"]["compilerBackend"],
        serde_json::json!("auto")
    );
}
