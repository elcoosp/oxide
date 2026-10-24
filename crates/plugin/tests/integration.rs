//! TDD suite for oxide-fw-plugin (spec §15).
//!
//! Covers: registration with bridge-capability enforcement (§15.2 "hooks the
//! bridge does not support fail at load"), same-name replacement, hook chain
//! ordering (§15.1), WASM manifest validation (§15.1 kind 2, §17.2 sandboxing)
//! and the JS bridge protocol messages.

use oxide_fw_plugin::*;

fn plugin(
    name: &str,
    kind: PluginKind,
    enforce: Option<Enforce>,
    hooks: Vec<Hook>,
) -> PluginDescriptor {
    PluginDescriptor {
        name: name.into(),
        kind,
        enforce,
        hooks,
        options: serde_json::json!({}),
        supported_adapters: vec![],
    }
}

// ------------------------------------------------------------ registration ---

#[test]
fn native_plugins_may_declare_bridge_unsupported_hooks() {
    let mut r = PluginRegistry::default();
    assert!(!r.has_errors());
    r.register(plugin(
        "oxide:tailwind",
        PluginKind::Native,
        None,
        vec![Hook::ConfigureServer, Hook::Transform],
    ));
    assert_eq!(r.plugins.len(), 1, "native plugins bypass the bridge check");
    assert!(!r.has_errors());
}

#[test]
fn unsupported_hook_fails_at_load_with_compat_table_link() {
    let mut r = PluginRegistry::default();
    r.register(plugin(
        "my-vite-plugin",
        PluginKind::Js,
        None,
        vec![Hook::Transform, Hook::ConfigureServer],
    ));
    assert!(
        r.plugins.is_empty(),
        "plugin with an unsupported hook is NOT added"
    );
    assert!(r.has_errors());
    assert_eq!(r.load_errors.len(), 1);
    let d = &r.load_errors[0];
    assert_eq!(d.code, "PLG-HOOK");
    assert!(
        d.message.contains("my-vite-plugin"),
        "error names the plugin"
    );
    assert!(
        d.message.contains("configureServer"),
        "error names the hook"
    );
    assert!(
        d.message.contains("15.2"),
        "error links the compatibility table (§15.2/15.3)"
    );
    // a well-behaved JS plugin still registers after the failure
    r.register(plugin(
        "mdx",
        PluginKind::Js,
        None,
        vec![Hook::Transform, Hook::Load],
    ));
    assert_eq!(r.plugins.len(), 1);
    assert_eq!(r.plugins[0].name, "mdx");
}

#[test]
fn wasm_kind_is_also_bridge_limited() {
    let mut r = PluginRegistry::default();
    r.register(plugin(
        "wasm-img",
        PluginKind::Wasm,
        None,
        vec![Hook::CacheHandler],
    ));
    assert!(
        r.plugins.is_empty(),
        "only Native bypasses the bridge check"
    );
    assert_eq!(r.load_errors[0].code, "PLG-HOOK");
}

#[test]
fn register_same_name_replaces_in_place() {
    let mut r = PluginRegistry::default();
    r.register(plugin("mdx", PluginKind::Js, None, vec![Hook::Transform]));
    r.register(plugin(
        "tailwind",
        PluginKind::Native,
        None,
        vec![Hook::TransformHtml],
    ));
    r.register(plugin(
        "mdx",
        PluginKind::Js,
        Some(Enforce::Pre),
        vec![Hook::Load, Hook::Transform],
    ));
    assert_eq!(
        r.plugins.len(),
        2,
        "duplicate name is replaced, not appended"
    );
    assert_eq!(
        r.plugins[0].name, "mdx",
        "replacement keeps the registration slot"
    );
    assert_eq!(r.plugins[0].hooks, vec![Hook::Load, Hook::Transform]);
    assert_eq!(r.plugins[0].enforce, Some(Enforce::Pre));
    assert_eq!(r.plugins[1].name, "tailwind");
    assert!(!r.has_errors(), "replacement is silent");
}

// ------------------------------------------------------------------ chain ----

#[test]
fn chain_orders_pre_then_default_then_post_stable() {
    let mut r = PluginRegistry::default();
    r.register(plugin(
        "p1-post",
        PluginKind::Js,
        Some(Enforce::Post),
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "p2-none",
        PluginKind::Js,
        None,
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "p3-pre",
        PluginKind::Js,
        Some(Enforce::Pre),
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "p4-none",
        PluginKind::Js,
        None,
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "p5-pre-load",
        PluginKind::Js,
        Some(Enforce::Pre),
        vec![Hook::Load],
    ));
    let names: Vec<&str> = r
        .chain(Hook::Transform)
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, vec!["p3-pre", "p2-none", "p4-none", "p1-post"]);
    // only p5 declares `load`; it is first overall (pre group)
    let load_names: Vec<&str> = r
        .chain(Hook::Load)
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(load_names, vec!["p5-pre-load"]);
    // a hook nobody declares yields an empty chain
    assert!(r.chain(Hook::Diagnostics).is_empty());
}

#[test]
fn chain_includes_native_and_wasm_kinds() {
    let mut r = PluginRegistry::default();
    r.register(plugin(
        "native",
        PluginKind::Native,
        None,
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "wasm",
        PluginKind::Wasm,
        Some(Enforce::Pre),
        vec![Hook::Transform],
    ));
    r.register(plugin(
        "js",
        PluginKind::Js,
        Some(Enforce::Post),
        vec![Hook::Transform],
    ));
    let names: Vec<&str> = r
        .chain(Hook::Transform)
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, vec!["wasm", "native", "js"]);
}

// --------------------------------------------------------- wasm manifests ----

fn manifest(wit_world: &str, capabilities: &[&str], hooks: Vec<Hook>) -> WasmPluginManifest {
    WasmPluginManifest {
        name: "acme-img".into(),
        wit_world: wit_world.into(),
        capabilities: capabilities.iter().map(|s| s.to_string()).collect(),
        hooks,
    }
}

#[test]
fn wasm_manifest_accepts_valid_input() {
    let m = manifest(
        "oxide:plugin/transform@0.1.0",
        &["fs", "network"],
        vec![Hook::Transform, Hook::Load],
    );
    m.validate().unwrap();
}

#[test]
fn wasm_manifest_rejects_unknown_capability() {
    let m = manifest("w", &["fs", "gpu"], vec![Hook::Transform]);
    let e = m.validate().unwrap_err();
    assert_eq!(e.0.code, "PLG-CAP");
    assert!(
        e.0.message.contains("gpu"),
        "error names the offending capability"
    );
    assert!(
        e.0.message.contains("17.2"),
        "error cites the sandboxing section"
    );
    // the full §17.2 sandbox set is accepted
    manifest(
        "w",
        &["fs", "network", "env", "clock"],
        vec![Hook::Transform],
    )
    .validate()
    .unwrap();
}

#[test]
fn wasm_manifest_rejects_empty_wit_world_or_hooks() {
    let e = manifest("", &["fs"], vec![Hook::Transform])
        .validate()
        .unwrap_err();
    assert_eq!(e.0.code, "PLG-WIT");
    let e2 = manifest("w", &[], vec![]).validate().unwrap_err();
    assert_eq!(e2.0.code, "PLG-HOOKS");
}

#[test]
fn wasm_manifest_rejects_bridge_unsupported_hook() {
    let m = manifest("w", &["fs"], vec![Hook::Transform, Hook::CacheHandler]);
    let e = m.validate().unwrap_err();
    assert_eq!(e.0.code, "PLG-HOOK");
    assert!(e.0.message.contains("cacheHandler"));
    assert!(
        e.0.message.contains("15.2"),
        "WASM runs the same bridge subset (§15.2/15.3)"
    );
}

// ------------------------------------------------------------------ bridge ---

#[test]
fn bridge_msg_json_roundtrip() {
    let call = BridgeMsg::HookCall {
        plugin: "mdx".into(),
        hook: "transform".into(),
        args: serde_json::json!({"code": "export default 1", "id": "a.md"}),
    };
    let s = serde_json::to_string(&call).unwrap();
    assert!(s.contains(r#""t":"hook_call""#), "tagged envelope: {s}");
    let back: BridgeMsg = serde_json::from_str(&s).unwrap();
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::to_value(&call).unwrap()
    );

    let result = BridgeMsg::HookResult {
        plugin: "mdx".into(),
        hook: "transform".into(),
        ok: false,
        result: serde_json::json!({"code": "PLG-JS", "message": "compile failed"}),
    };
    let s2 = serde_json::to_string(&result).unwrap();
    assert!(s2.contains(r#""t":"hook_result""#), "tagged envelope: {s2}");
    let back2: BridgeMsg = serde_json::from_str(&s2).unwrap();
    assert_eq!(
        serde_json::to_value(&back2).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
}
