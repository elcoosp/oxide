//! Walks the plugin-compat matrix end to end (the E05 substrate seam):
//! renders the machine-checked compatibility table, loads a WASM component
//! that speaks the `load`/`resolveId` ABI, exercises both hooks plus the
//! object-safe [`HookHost`] seam, and prints the `unsupportedHooks` list the
//! compatibility runner records.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-plugin --example compat_matrix
//! ```

use oxide_fw_plugin::{Hook, HookHost, WasmPlugin, WasmPluginManifest, compat_markdown_table};

/// `resolve_id: func(specifier: string, from: string) -> option<string>` —
/// rewrites `./missing.ts` to a virtual shim, declines everything else (the
/// pass-to-default arm). Static data segment, canonical return area.
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

fn main() {
    println!("== the published hooks table (compat_markdown_table()) ==");
    println!("{}", compat_markdown_table());

    let manifest = WasmPluginManifest {
        name: "shim-resolver".into(),
        wit_world: "oxide:plugin/hooks@0.1.0".into(),
        capabilities: vec![],
        // Declares `load` too: the component does not export it, so the
        // manifest-vs-export gap is visible in the host's report.
        hooks: vec![Hook::ResolveId, Hook::Load],
    };
    let plugin = WasmPlugin::load(RESOLVE_WAT.as_bytes(), manifest, None).expect("component loads");

    println!("== the host's hook report ==");
    println!(
        "available (callable shapes): {:?}",
        plugin.available_hooks()
    );
    println!("missing (unsupportedHooks):  {:?}", plugin.missing_hooks());

    println!("== resolve_id through the object-safe seam ==");
    let mut host: Box<dyn HookHost> = Box::new(plugin);
    let rewritten = host
        .call_resolve_id("./missing.ts", "/app/src/page.tsx")
        .expect("resolve_id call");
    println!("./missing.ts  → {rewritten:?}  (Some: the plugin's id wins)");
    let declined = host
        .call_resolve_id("", "/app/src/page.tsx")
        .expect("resolve_id call");
    println!("(decline)     → {declined:?}  (None: the default resolver wins)");

    println!();
    println!("the remaining half: the CLI wire that feeds these into");
    println!("BundleOptions.load_hook / resolve_hook (see docs/plugin-compat.md).");
}
