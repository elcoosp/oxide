//! TDD suite for the plugin-compat matrix and the WASM `load`/`resolveId` host
//! ABI (SPEC-15.2/§15.3, the E05 substrate half).
//!
//! Two surfaces, one red-first suite:
//!
//! 1. the machine-checked compatibility matrix — `compat(kind, hook)` and the
//!    deterministic `compat_markdown_table()` the published table embeds
//!    (the E11 `doc_table_matches_the_function` pattern, applied to
//!    `docs/plugin-compat.md`);
//! 2. the WASM host's `load`/`resolveId` call paths — the two hooks the
//!    compatibility table carries as 🟡: the host can call them (string
//!    fixtures lifted over the canonical ABI, including the option-return
//!    shapes), the CLI call site that feeds them into the bundler's
//!    `BundleOptions.load_hook`/`resolve_hook` is the remaining half.
//!
//! The WAT fixtures follow `wasm_tdd.rs`'s hand-written-component pattern;
//! string-carrying signatures ride static data segments (no allocation), and
//! `option<string>` results ride the canonical return area the canon lift
//! provides.

use oxide_fw_plugin::{
    Hook, HookHost, LoadTimeReport, PluginKind, PluginRegistry, SupportLevel, WasmPlugin,
    WasmPluginManifest, compat, compat_markdown_table, host_invocable_hooks, unsupported_hooks,
};

/// `unwrap_err` needs the Ok type to be `Debug`; wasmtime handles are not.
fn err_of<T>(r: oxide_fw_plugin::Result<T>) -> oxide_fw_diagnostics::OxideError {
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

// ---------- the compatibility matrix ----------

/// The doc's hooks-table order (the matrix iterates it, so the generated
/// table can never silently re-sort).
const DOC_ORDER: [Hook; 19] = [
    Hook::Config,
    Hook::ConfigResolved,
    Hook::BuildStart,
    Hook::BuildEnd,
    Hook::ResolveId,
    Hook::Load,
    Hook::Transform,
    Hook::RenderChunk,
    Hook::GenerateBundle,
    Hook::TransformHtml,
    Hook::HandleHotUpdate,
    Hook::Routes,
    Hook::Typegen,
    Hook::Diagnostics,
    Hook::ServerMiddleware,
    Hook::ConfigureServer,
    Hook::Adapter,
    Hook::RenderHead,
    Hook::CacheHandler,
];

#[test]
fn matrix_covers_every_hook_in_the_doc_order() {
    assert_eq!(oxide_fw_plugin::HOOKS, DOC_ORDER);
}

#[test]
fn matrix_wasm_transform_is_wired() {
    assert_eq!(
        compat(PluginKind::Wasm, Hook::Transform),
        SupportLevel::Wired
    );
}

#[test]
fn matrix_wasm_load_and_resolve_id_are_host_callable_only() {
    // The host can call both (this suite proves the ABI below); the CLI call
    // site that feeds them into the bundler hooks is the remaining half.
    assert_eq!(compat(PluginKind::Wasm, Hook::Load), SupportLevel::Partial);
    assert_eq!(
        compat(PluginKind::Wasm, Hook::ResolveId),
        SupportLevel::Partial
    );
}

#[test]
fn matrix_bridge_unsupported_hooks_are_unsupported_for_every_non_native_kind() {
    for kind in [PluginKind::Js, PluginKind::Wasm] {
        assert_eq!(
            compat(kind, Hook::ConfigureServer),
            SupportLevel::Unsupported,
            "{kind:?} configureServer"
        );
        assert_eq!(
            compat(kind, Hook::CacheHandler),
            SupportLevel::Unsupported,
            "{kind:?} cacheHandler"
        );
    }
}

#[test]
fn matrix_native_is_not_applicable() {
    for hook in DOC_ORDER {
        assert_eq!(
            compat(PluginKind::Native, hook),
            SupportLevel::NotApplicable,
            "native {hook:?}"
        );
    }
}

#[test]
fn matrix_js_bridge_supported_hooks_are_wired() {
    for hook in [
        Hook::Transform,
        Hook::Load,
        Hook::ResolveId,
        Hook::Routes,
        Hook::Typegen,
    ] {
        assert_eq!(
            compat(PluginKind::Js, hook),
            SupportLevel::Wired,
            "{hook:?}"
        );
    }
}

#[test]
fn compat_markdown_table_is_deterministic() {
    assert_eq!(compat_markdown_table(), compat_markdown_table());
}

#[test]
fn table_covers_every_kind_cell() {
    // 19 hook rows × 3 kind columns: every cell renders one of the four
    // markers, and no row ever drops a kind.
    let table = compat_markdown_table();
    for hook in DOC_ORDER {
        let marker = |level: SupportLevel| -> &'static str {
            match level {
                SupportLevel::Wired => "✅",
                SupportLevel::Partial => "🟡",
                SupportLevel::Unsupported => "🚫",
                SupportLevel::NotApplicable => "n/a",
            }
        };
        for kind in [PluginKind::Js, PluginKind::Wasm, PluginKind::Native] {
            assert!(
                table.contains(marker(compat(kind, hook))),
                "{kind:?} {hook:?} missing its marker from the table"
            );
        }
    }
}

#[test]
fn plugin_compat_doc_embeds_the_generated_table() {
    let doc = include_str!("../../../docs/plugin-compat.md");
    assert!(
        doc.contains(&compat_markdown_table()),
        "docs/plugin-compat.md's hooks table has drifted from \
         compat_markdown_table() — regenerate and re-embed the exact output"
    );
}

// ---------- the WASM load/resolveId ABI ----------

/// `load: func(specifier: string) -> option<string>`. Empty specifier →
/// `none` (decline); anything else → `some` over a static data segment.
/// `option<string>` flattens to 3 i32s, so the canonical return area carries
/// the answer: `(disc, ptr, len)` written through the out-pointer the lift
/// appends.
const LOAD_WAT: &str = r#"
(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 8) "\00\00\00\00\00\00\00\00\00\00\00\00")
    (data (i32.const 24) "\01\00\00\00" "\28\00\00\00" "\15\00\00\00")
    (data (i32.const 40) "virtual-module-source")
    (global $bump (mut i32) (i32.const 128))
    (func (export "cabi_realloc") (param $o i32) (param $os i32) (param $a i32) (param $ns i32)
      (result i32)
      (local $ret i32)
      (local.set $ret (global.get $bump))
      (global.set $bump (i32.add (global.get $bump) (i32.const 64)))
      (local.get $ret)
    )
    (func (export "load_dyn") (param $p i32) (param $l i32) (result i32)
      (if (result i32) (i32.eqz (local.get $l))
        (then (i32.const 8))
        (else (i32.const 24))
      )
    )
  )
  (core instance $inst (instantiate $m))
  (alias core export $inst "memory" (core memory $mem))
  (alias core export $inst "cabi_realloc" (core func $realloc))
  (alias core export $inst "load_dyn" (core func $f))
  (type $sig (func (param "specifier" string) (result (option string))))
  (func $lifted (type $sig) (canon lift (core func $f) (memory $mem) (realloc $realloc)))
  (export "load" (func $lifted))
)
"#;

#[test]
fn wasm_load_hook_declines_with_none() {
    let mut plugin =
        WasmPlugin::load(LOAD_WAT.as_bytes(), manifest(vec![Hook::Load]), None).unwrap();
    assert!(plugin.available_hooks().contains(&Hook::Load));
    assert_eq!(plugin.call_load_hook("").unwrap(), None, "decline → None");
}

#[test]
fn wasm_load_hook_serves_a_virtual_module() {
    let mut plugin =
        WasmPlugin::load(LOAD_WAT.as_bytes(), manifest(vec![Hook::Load]), None).unwrap();
    assert_eq!(
        plugin.call_load_hook("./icon.svg").unwrap(),
        Some("virtual-module-source".to_string())
    );
}

/// `resolve_id: func(specifier: string, from: string) -> option<string>`.
/// The two string params flatten to 4 i32s; the out-pointer appends last.
const RESOLVE_WAT: &str = r#"
(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 8) "\00\00\00\00\00\00\00\00\00\00\00\00")
    (data (i32.const 24) "\01\00\00\00" "\28\00\00\00" "\12\00\00\00")
    (data (i32.const 40) "../virtual/shim.ts")
    (global $bump (mut i32) (i32.const 128))
    (func (export "cabi_realloc") (param $o i32) (param $os i32) (param $a i32) (param $ns i32)
      (result i32)
      (local $ret i32)
      (local.set $ret (global.get $bump))
      (global.set $bump (i32.add (global.get $bump) (i32.const 64)))
      (local.get $ret)
    )
    (func (export "resolve_dyn")
      (param $p1 i32) (param $l1 i32) (param $p2 i32) (param $l2 i32)
      (result i32)
      (if (result i32) (i32.eqz (local.get $l1))
        (then (i32.const 8))
        (else (i32.const 24))
      )
    )
  )
  (core instance $inst (instantiate $m))
  (alias core export $inst "memory" (core memory $mem))
  (alias core export $inst "cabi_realloc" (core func $realloc))
  (alias core export $inst "resolve_dyn" (core func $f))
  (type $sig (func (param "specifier" string) (param "from" string) (result (option string))))
  (func $lifted (type $sig) (canon lift (core func $f) (memory $mem) (realloc $realloc)))
  (export "resolve-id" (func $lifted))
)
"#;

#[test]
fn wasm_resolve_hook_rewrites_the_specifier() {
    let mut plugin = WasmPlugin::load(
        RESOLVE_WAT.as_bytes(),
        manifest(vec![Hook::ResolveId]),
        None,
    )
    .unwrap();
    assert_eq!(
        plugin
            .call_resolve_hook("./missing.ts", "/app/src/page.ts")
            .unwrap(),
        Some("../virtual/shim.ts".to_string())
    );
}

#[test]
fn wasm_resolve_hook_none_passes_to_default_resolution() {
    let mut plugin = WasmPlugin::load(
        RESOLVE_WAT.as_bytes(),
        manifest(vec![Hook::ResolveId]),
        None,
    )
    .unwrap();
    assert_eq!(
        plugin.call_resolve_hook("", "/app/src/page.ts").unwrap(),
        None,
        "empty specifier declines → the bundler's default resolver wins"
    );
}

/// `transform: func(code: string) -> string` over a static return — the
/// real string-shape exercise the s32 fixture cannot provide.
const TRANSFORM_STRING_WAT: &str = r#"
(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 8) "\28\00\00\00" "\05\00\00\00")
    (data (i32.const 40) "UPPER")
    (global $bump (mut i32) (i32.const 128))
    (func (export "cabi_realloc") (param $o i32) (param $os i32) (param $a i32) (param $ns i32)
      (result i32)
      (local $ret i32)
      (local.set $ret (global.get $bump))
      (global.set $bump (i32.add (global.get $bump) (i32.const 64)))
      (local.get $ret)
    )
    (func (export "transform_dyn") (param $p i32) (param $l i32) (result i32)
      (i32.const 8)
    )
  )
  (core instance $inst (instantiate $m))
  (alias core export $inst "memory" (core memory $mem))
  (alias core export $inst "cabi_realloc" (core func $realloc))
  (alias core export $inst "transform_dyn" (core func $f))
  (type $sig (func (param "code" string) (result string)))
  (func $lifted (type $sig) (canon lift (core func $f) (memory $mem) (realloc $realloc)))
  (export "transform" (func $lifted))
)
"#;

#[test]
fn hook_host_object_safe_roundtrip() {
    // The seam the CLI wire programs against: a boxed host, every call path
    // through the trait (transform's string shape, load, resolve_id).
    let mut host: Box<dyn HookHost> = Box::new(
        WasmPlugin::load(
            TRANSFORM_STRING_WAT.as_bytes(),
            manifest(vec![Hook::Transform]),
            None,
        )
        .unwrap(),
    );
    assert_eq!(host.call_transform("input").unwrap(), "UPPER");

    let mut host: Box<dyn HookHost> =
        Box::new(WasmPlugin::load(LOAD_WAT.as_bytes(), manifest(vec![Hook::Load]), None).unwrap());
    assert_eq!(
        host.call_load("./icon.svg").unwrap(),
        Some("virtual-module-source".to_string())
    );

    let mut host: Box<dyn HookHost> = Box::new(
        WasmPlugin::load(
            RESOLVE_WAT.as_bytes(),
            manifest(vec![Hook::ResolveId]),
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        host.call_resolve_id("./missing.ts", "/app/src/page.ts")
            .unwrap(),
        Some("../virtual/shim.ts".to_string())
    );
}

#[test]
fn missing_hooks_lists_declared_unexported() {
    // The component exports `load` only; the manifest declares both `load`
    // and `transform`. `transform` is declared-but-unexported — exactly the
    // list the spec's compat runner records as `unsupportedHooks`.
    let plugin = WasmPlugin::load(
        LOAD_WAT.as_bytes(),
        manifest(vec![Hook::Load, Hook::Transform]),
        None,
    )
    .unwrap();
    assert_eq!(plugin.missing_hooks(), vec![Hook::Transform]);
}

/// The s32 fixture from `wasm_tdd.rs` (each integration test is its own
/// crate, so the const is duplicated here): `(s32) -> s32` — every shape the
/// new string paths must refuse cleanly.
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
fn wasm_wrong_shape_load_errors_cleanly() {
    // The s32 `transform` fixture declaring `load`: the export exists but the
    // shape is not `(string) -> option<string>`, so the typed-func resolution
    // refuses with PLG-WASM naming the hook.
    let mut plugin =
        WasmPlugin::load(ADD_ONE_WAT.as_bytes(), manifest(vec![Hook::Load]), None).unwrap();
    let e = err_of(plugin.call_load_hook("./x.ts"));
    assert_eq!(e.0.code, "PLG-WASM");
    assert!(e.0.message.contains("load"), "{}", e.0.message);
}

// ---------- V2-E05: the load-time unsupported-hook report ----------

/// Build a [`oxide_fw_plugin::PluginDescriptor`] for the registry pins.
fn descriptor(name: &str, kind: PluginKind, hooks: Vec<Hook>) -> oxide_fw_plugin::PluginDescriptor {
    oxide_fw_plugin::PluginDescriptor {
        name: name.into(),
        kind,
        enforce: None,
        hooks,
        options: serde_json::json!({}),
        supported_adapters: vec![],
    }
}

#[test]
fn host_invocable_hooks_matches_the_compat_model() {
    // The host can invoke exactly the Wired + Partial cells of `compat`.
    let js = host_invocable_hooks(PluginKind::Js);
    assert!(js.contains(&Hook::Transform));
    assert!(js.contains(&Hook::ResolveId));
    assert!(!js.contains(&Hook::ConfigureServer));
    assert!(!js.contains(&Hook::CacheHandler));
    let wasm = host_invocable_hooks(PluginKind::Wasm);
    assert_eq!(wasm.len(), 3, "wasm host invokes transform/load/resolveId");
    assert!(wasm.contains(&Hook::Transform));
    assert!(wasm.contains(&Hook::Load));
    assert!(wasm.contains(&Hook::ResolveId));
    assert!(
        host_invocable_hooks(PluginKind::Native).is_empty(),
        "native plugins never cross the bridge"
    );
}

#[test]
fn unsupported_hooks_names_the_bridge_refused_ones_for_js() {
    // A JS plugin declaring `configureServer` + `transform`: the bridge
    // refuses `configureServer` (PLG-HOOK); the model names it unsupported.
    let declared = [Hook::ConfigureServer, Hook::Transform];
    assert_eq!(
        unsupported_hooks(PluginKind::Js, &declared, &declared),
        vec![Hook::ConfigureServer]
    );
    // A clean JS plugin (no bridge-unsupported hook) reports nothing.
    let clean = [Hook::Transform, Hook::ResolveId, Hook::Load];
    assert!(unsupported_hooks(PluginKind::Js, &clean, &clean).is_empty());
}

#[test]
fn unsupported_hooks_folds_both_wasm_gaps() {
    // Gap 1: declared-but-unexported (`buildStart` is not in `available`).
    // Gap 2: exported-but-no-host-path is covered by the same filter — a
    // declared hook not in the host-invocable set {transform,load,resolveId}.
    // Here `buildStart` is both unexported AND not host-invocable.
    let declared = [Hook::Transform, Hook::BuildStart];
    let available = [Hook::Transform];
    assert_eq!(
        unsupported_hooks(PluginKind::Wasm, &declared, &available),
        vec![Hook::BuildStart]
    );
}

#[test]
fn unsupported_hooks_is_empty_for_native() {
    // Native plugins never cross the bridge — the report is a no-op.
    let declared = [Hook::Transform, Hook::ConfigureServer];
    assert!(unsupported_hooks(PluginKind::Native, &declared, &declared).is_empty());
}

#[test]
fn load_time_report_partitions_declared_hooks() {
    let r = LoadTimeReport::new(
        PluginKind::Js,
        "p",
        &[Hook::Transform, Hook::ConfigureServer, Hook::CacheHandler],
        &[Hook::Transform, Hook::ConfigureServer, Hook::CacheHandler],
    );
    assert_eq!(
        r.unsupported_hooks,
        vec![Hook::ConfigureServer, Hook::CacheHandler]
    );
    assert_eq!(r.wired_hooks, vec![Hook::Transform]);
    assert!(!r.is_fully_wired());
    // A fully-wired plugin reports no gap.
    let clean = LoadTimeReport::new(PluginKind::Js, "p", &[Hook::Transform], &[Hook::Transform]);
    assert!(clean.is_fully_wired());
}

#[test]
fn load_time_report_json_uses_camel_case_hook_names() {
    // The runner's `parseReport` reads camelCase hook names + the
    // `unsupportedHooks`/`wiredHooks` field names the spec names.
    let r = LoadTimeReport::new(
        PluginKind::Js,
        "vite-plugin-x",
        &[Hook::Transform, Hook::ConfigureServer],
        &[Hook::Transform, Hook::ConfigureServer],
    );
    let json = r.to_json();
    assert!(json.contains("\"plugin\":\"vite-plugin-x\""), "{json}");
    assert!(json.contains("\"kind\":\"js\""), "{json}");
    assert!(
        json.contains("\"unsupportedHooks\":[\"configureServer\"]"),
        "{json}"
    );
    assert!(json.contains("\"wiredHooks\":[\"transform\"]"), "{json}");
}

#[test]
fn wasm_load_time_report_records_the_silent_gap() {
    // A component exporting `transform` only, declaring `transform` +
    // `buildStart`: `buildStart` has no host call path → the report names
    // it (not silently dropped). This is the V2-E05 acceptance: a plugin
    // declaring a hook the host does not invoke is reported at load time.
    let plugin = WasmPlugin::load(
        TRANSFORM_STRING_WAT.as_bytes(),
        manifest(vec![Hook::Transform, Hook::BuildStart]),
        None,
    )
    .unwrap();
    let report = plugin.load_time_report();
    assert_eq!(report.kind, PluginKind::Wasm);
    assert_eq!(report.unsupported_hooks, vec![Hook::BuildStart]);
    assert_eq!(report.wired_hooks, vec![Hook::Transform]);
}

#[test]
fn registry_records_a_load_report_per_accepted_plugin() {
    // `register` lands a report even for a clean plugin (the SILENT gap
    // substrate); same-name replacement overwrites the report in place.
    let mut reg = PluginRegistry::default();
    reg.register(descriptor("a", PluginKind::Js, vec![Hook::Transform]));
    reg.register(descriptor(
        "b",
        PluginKind::Js,
        vec![Hook::ResolveId, Hook::Load],
    ));
    let reports = reg.load_reports();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].plugin, "a");
    assert!(reports[0].is_fully_wired());
    assert_eq!(reports[1].plugin, "b");
    assert!(reports[1].is_fully_wired());
    // Replacement overwrites in place (the registry's slot rule).
    reg.register(descriptor("a", PluginKind::Js, vec![Hook::ConfigureServer]));
    // `configureServer` fails the bridge check → not added, but the prior
    // report for `a` is unchanged (register returns before recording).
    assert_eq!(reg.load_reports().len(), 2);
    assert_eq!(reg.load_reports()[0].plugin, "a");
}

#[test]
fn registry_emit_load_reports_jsonl_carries_the_prefix() {
    let mut reg = PluginRegistry::default();
    reg.register(descriptor("a", PluginKind::Js, vec![Hook::Transform]));
    let out = reg.emit_load_reports_jsonl();
    assert!(
        out.starts_with("oxide-plugin-load\t"),
        "the runner scans the build log for this prefix"
    );
    assert!(out.contains("\"plugin\":\"a\""), "{out}");
    assert!(out.contains("\"unsupportedHooks\":[]"), "{out}");
}
