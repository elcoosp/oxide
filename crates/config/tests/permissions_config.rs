//! V2-F19 (§V2-F19): the `permissions` config section — typed field,
//! default posture, allow-lists, schema declaration and the `CFG-PERM`
//! raw-`Value` validator. Mirrors the `offline_config` TDD suite's shape.

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

/// Load one config body (with the required `framework.adapter`); the
/// error is asserted through `err.0.code`.
fn load_body(
    body: &str,
) -> Result<oxide_fw_config::LoadedConfig, oxide_fw_diagnostics::OxideError> {
    let (_t, dir) = fixture(&[("oxide.config.json", body)]);
    load_with_env(&dir, "production", BTreeMap::new(), &EMPTY_ENV)
}

/// The minimal valid body — `framework.adapter` is required by the
/// loader; every test's body wraps its section inside this shell.
fn shell(inner: &str) -> String {
    format!(r#"{{ "framework": {{ "adapter": "react" }}, {} }}"#, inner)
}

// ---------------------------------------------------------------- shape

#[test]
fn permissions_defaults_match_the_spec_shape() {
    // §V2-F19: `permissions?: { default?: 'allow' | 'deny'; net?: string[]
    // | null; env?: string[] | null; fs?: string[] | null }` — allow-all
    // by default (v0.2), every field `None` (unrestricted).
    let cfg = OxideConfig::default();
    assert_eq!(
        cfg.permissions.default,
        oxide_fw_jshost::perm::PermissionDefault::Allow
    );
    assert!(cfg.permissions.net.is_none());
    assert!(cfg.permissions.env.is_none());
    assert!(cfg.permissions.fs.is_none());
}

#[test]
fn permissions_section_parses_camel_case() {
    let cfg = load_body(&shell(
        r#""permissions": {
                "default": "deny",
                "net": ["api.stripe.com", "*.internal"],
                "env": ["STRIPE_KEY"],
                "fs": ["./data"]
            }"#,
    ))
    .expect("parses");
    let p = &cfg.config.permissions;
    assert_eq!(p.default, oxide_fw_jshost::perm::PermissionDefault::Deny);
    assert_eq!(
        p.net.as_ref().map(Vec::as_slice),
        Some(["api.stripe.com".to_string(), "*.internal".to_string()].as_slice())
    );
    assert_eq!(
        p.env.as_ref().map(Vec::as_slice),
        Some(["STRIPE_KEY".to_string()].as_slice())
    );
    assert_eq!(
        p.fs.as_ref().map(Vec::as_slice),
        Some(["./data".to_string()].as_slice())
    );
}

#[test]
fn permissions_null_fields_are_unrestricted() {
    // `null` on a field is unrestricted (distinct from `[]` deny-all).
    let cfg = load_body(&shell(
        r#""permissions": { "default": "allow", "net": null, "env": null, "fs": null }"#,
    ))
    .expect("parses");
    let p = &cfg.config.permissions;
    assert!(p.net.is_none());
    assert!(p.env.is_none());
    assert!(p.fs.is_none());
}

#[test]
fn permissions_empty_array_is_deny_all_for_that_field() {
    let cfg = load_body(&shell(r#""permissions": { "net": [] }"#)).expect("parses");
    let p = &cfg.config.permissions;
    assert_eq!(p.net.as_ref().map(Vec::as_slice), Some([].as_slice()));
}

// ----------------------------------------------------- CFG-PERM errors

#[test]
fn bad_default_spelling_is_cfg_perm() {
    let err = load_body(&shell(r#""permissions": { "default": "yes" }"#)).unwrap_err();
    assert_eq!(err.0.code, "CFG-PERM");
}

#[test]
fn non_array_net_is_cfg_perm() {
    let err = load_body(&shell(r#""permissions": { "net": "api.stripe.com" }"#)).unwrap_err();
    assert_eq!(err.0.code, "CFG-PERM");
}

#[test]
fn non_string_net_item_is_cfg_perm() {
    let err = load_body(&shell(r#""permissions": { "net": [42] }"#)).unwrap_err();
    assert_eq!(err.0.code, "CFG-PERM");
}

#[test]
fn unknown_key_is_cfg_perm() {
    // `deny_unknown_fields` on `PermissionsConfig` rejects unknown keys.
    let err = load_body(&shell(r#""permissions": { "network": ["x"] }"#)).unwrap_err();
    assert_eq!(err.0.code, "CFG-PERM");
}

// ---------------------------------------------------- known key + schema

#[test]
fn permissions_is_a_known_top_level_key_for_typo_diagnostics() {
    // A typo'd `permission` (singular) should surface the CFG-UNKNOWN-KEY
    // suggestion pointing at `permissions`, not a silent accept.
    let err = load_body(&shell(r#""permission": { "default": "allow" }"#)).unwrap_err();
    assert_eq!(err.0.code, "CFG-UNKNOWN-KEY");
}

#[test]
fn schema_declares_the_permissions_section() {
    let schema = config_schema();
    let perms = &schema["properties"]["permissions"];
    assert_eq!(perms["type"], "object");
    let props = perms
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("permissions schema has a properties object");
    for key in ["default", "net", "env", "fs"] {
        assert!(
            props.contains_key(key),
            "permissions.{key} missing from schema"
        );
    }
    assert_eq!(props["default"]["enum"][0], "allow");
    assert_eq!(props["default"]["enum"][1], "deny");
}

// ----------------------------------------------- to_permission_set bridge

#[test]
fn to_permission_set_carries_the_allow_lists() {
    // The bridge the dispatcher reads: `PermissionsConfig` → the typed
    // `PermissionSet` the op boundary checks consult.
    let cfg = load_body(&shell(
        r#""permissions": {
                "default": "deny",
                "net": ["api.stripe.com"],
                "env": ["STRIPE_KEY"],
                "fs": ["./data"]
            }"#,
    ))
    .expect("parses");
    let set = cfg.config.permissions.to_permission_set();
    assert!(set.is_net_allowed("api.stripe.com"));
    assert!(!set.is_net_allowed("evil.example"));
    assert!(set.is_env_allowed("STRIPE_KEY"));
    assert!(!set.is_env_allowed("DATABASE_URL"));
    assert!(set.is_fs_allowed("./data/users.json"));
    assert!(!set.is_fs_allowed("./secrets"));
}
