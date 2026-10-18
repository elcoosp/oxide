//! TDD suite for the real WASM component plugin host (spec §15.1 kind 2, §17.2).
//!
//! Uses wasmtime 48's component model. A hand-written WAT component that lifts a
//! core `(i32) -> i32` function into a component `(s32) -> s32` export exercises
//! the full `load → instantiate → probe → call → return` path end to end.

use oxide_plugin::{Hook, WasmPlugin, WasmPluginManifest, hook_export_name};

/// `unwrap_err` needs the Ok type to be `Debug`; wasmtime handles are not.
fn err_of<T>(r: oxide_plugin::Result<T>) -> oxide_diagnostics::OxideError {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

fn manifest(hooks: Vec<Hook>) -> WasmPluginManifest {
    WasmPluginManifest {
        name: "test-plugin".into(),
        wit_world: "oxide:plugin/hooks@0.1.0".into(),
        capabilities: vec![],
        hooks,
    }
}

// ---------- loading ----------

#[test]
fn loads_empty_component() {
    let plugin = WasmPlugin::load(b"(component)", manifest(vec![Hook::Transform]), None)
        .expect("empty component loads");
    assert_eq!(plugin.manifest().name, "test-plugin");
    assert!(plugin.available_hooks().is_empty());
}

#[test]
fn rejects_invalid_component_bytes() {
    let e = err_of(WasmPlugin::load(
        b"not a component at all",
        manifest(vec![Hook::Transform]),
        None,
    ));
    assert_eq!(e.0.code, "PLG-WASM");
    assert!(e.0.message.contains("failed to compile"), "{}", e.0.message);
}

#[test]
fn rejects_component_when_manifest_validation_fails() {
    let bad = WasmPluginManifest {
        name: "bad".into(),
        wit_world: String::new(),
        capabilities: vec![],
        hooks: vec![Hook::Transform],
    };
    let e = err_of(WasmPlugin::load(b"(component)", bad, None));
    assert_eq!(e.0.code, "PLG-WIT");
}

#[test]
fn rejects_component_when_no_hooks_declared() {
    let bad = WasmPluginManifest {
        name: "no-hooks".into(),
        wit_world: "oxide:plugin/hooks@0.1.0".into(),
        capabilities: vec![],
        hooks: vec![],
    };
    let e = err_of(WasmPlugin::load(b"(component)", bad, None));
    assert_eq!(e.0.code, "PLG-HOOKS");
}

#[test]
fn rejects_component_when_capability_not_granted() {
    let bad = WasmPluginManifest {
        name: "bad-cap".into(),
        wit_world: "oxide:plugin/hooks@0.1.0".into(),
        capabilities: vec!["root".into()],
        hooks: vec![Hook::Transform],
    };
    let e = err_of(WasmPlugin::load(b"(component)", bad, None));
    assert_eq!(e.0.code, "PLG-CAP");
}

// ---------- dispatch ----------

#[test]
fn call_unexported_hook_errors_with_plg_wasm() {
    let mut plugin =
        WasmPlugin::load(b"(component)", manifest(vec![Hook::Transform]), None).unwrap();
    let e = err_of(plugin.call_string_hook(&Hook::Transform, "input"));
    assert_eq!(e.0.code, "PLG-WASM");
    assert!(
        e.0.message.contains("does not export hook"),
        "{}",
        e.0.message
    );
    assert!(e.0.message.contains("transform"), "{}", e.0.message);
}

#[test]
fn hook_export_name_is_stable() {
    assert_eq!(hook_export_name(&Hook::Transform), "transform");
    assert_eq!(hook_export_name(&Hook::Load), "load");
    assert_eq!(hook_export_name(&Hook::ResolveId), "resolve_id");
    assert_eq!(hook_export_name(&Hook::Config), "config");
    assert_eq!(hook_export_name(&Hook::ConfigResolved), "config_resolved");
    assert_eq!(hook_export_name(&Hook::ConfigureServer), "configure_server");
}

// ---------- real execution (s32 roundtrip) ----------

/// Real component-model component. Lifts a core `(i32) -> i32` that adds 1 into a
/// component `(s32) -> s32` export. Fully parseable, real execution.
const ADD_ONE_WAT: &str = r#"
(component
  (core module $m
    (func (export "add_one") (param i32) (result i32)
      local.get 0
      i32.const 1
      i32.add
    )
  )
  (core instance $inst (instantiate $m))
  (alias core export $inst "add_one" (core func $f))
  (type $sig (func (param "x" s32) (result s32)))
  (func $lifted (type $sig) (canon lift (core func $f)))
  (export "transform" (func $lifted))
)
"#;

#[test]
fn real_component_transform_add_one_roundtrips() {
    let mut plugin = WasmPlugin::load(
        ADD_ONE_WAT.as_bytes(),
        manifest(vec![Hook::Transform]),
        None,
    )
    .expect("real component loads");
    assert!(
        plugin.available_hooks().contains(&Hook::Transform),
        "available hooks: {:?}",
        plugin.available_hooks()
    );
    assert_eq!(plugin.call_s32_hook(&Hook::Transform, 41).unwrap(), 42);
    assert_eq!(plugin.call_s32_hook(&Hook::Transform, 0).unwrap(), 1);
    assert_eq!(plugin.call_s32_hook(&Hook::Transform, -100).unwrap(), -99);
}

#[test]
fn real_component_honours_declared_hook_subset() {
    // Component exports `transform`; manifest declares `load` only. The declared
    // hook doesn't match, so `available_hooks` must be empty even though the
    // component has a `transform` export.
    let plugin =
        WasmPlugin::load(ADD_ONE_WAT.as_bytes(), manifest(vec![Hook::Load]), None).unwrap();
    assert!(plugin.available_hooks().is_empty());
}

#[test]
fn real_component_call_wrong_abi_shape_errors() {
    // Calling `call_string_hook` on an s32-signature export must error cleanly.
    let mut plugin = WasmPlugin::load(
        ADD_ONE_WAT.as_bytes(),
        manifest(vec![Hook::Transform]),
        None,
    )
    .unwrap();
    let e = err_of(plugin.call_string_hook(&Hook::Transform, "hello"));
    assert_eq!(e.0.code, "PLG-WASM");
    assert!(
        e.0.message.contains("resolve typed func") || e.0.message.contains("not"),
        "{}",
        e.0.message
    );
}
