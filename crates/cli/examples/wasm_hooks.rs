//! The §V2-E05 walkthrough: a `kind: wasm` component plugin's
//! `resolveId`/`load` hooks reach the real build — the CLI composes every
//! loaded component into the bundler's `BundleOptions.load_hook`/
//! `resolve_hook` beside the JS bridge (`build_wasm_load_resolve_hooks`,
//! the recorded remaining half of the E05 row).
//!
//! The walkthrough builds a tiny project whose config declares one
//! resolve component and one load component (hand-authored WAT — the
//! compat suite's fixture shape), constructs the SAME hook pair the build
//! and dev paths feed `BundleOptions`, then walks the wire end to end:
//! the resolve arm rewrites a specifier, the decline falls through, the
//! load arm serves the virtual module's source, the decline lets the disk
//! read win.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example wasm_hooks
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use oxide_fw_cli::build_wasm_load_resolve_hooks;

/// `resolve-id` over a static answer: non-empty specifier → `marker`;
/// empty → decline (the bundler's default resolver wins).
fn resolve_component_wat(marker: &str) -> String {
    let len = marker.len();
    let (b1, b2, b3, b4) = (
        len & 0xFF,
        (len >> 8) & 0xFF,
        (len >> 16) & 0xFF,
        (len >> 24) & 0xFF,
    );
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 8) "\00\00\00\00\00\00\00\00\00\00\00\00")
    (data (i32.const 24) "\01\00\00\00" "\28\00\00\00" "\{b1:02X}\{b2:02X}\{b3:02X}\{b4:02X}")
    (data (i32.const 40) "{marker}")
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
)"#
    )
}

/// `load` over a static answer: non-empty specifier → the virtual
/// module's source; empty → decline (the disk read wins).
fn load_component_wat(marker: &str) -> String {
    let len = marker.len();
    let (b1, b2, b3, b4) = (
        len & 0xFF,
        (len >> 8) & 0xFF,
        (len >> 16) & 0xFF,
        (len >> 24) & 0xFF,
    );
    format!(
        r#"(component
  (core module $m
    (memory (export "memory") 1)
    (data (i32.const 8) "\00\00\00\00\00\00\00\00\00\00\00\00")
    (data (i32.const 24) "\01\00\00\00" "\28\00\00\00" "\{b1:02X}\{b2:02X}\{b3:02X}\{b4:02X}")
    (data (i32.const 40) "{marker}")
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
)"#
    )
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    // The project: one resolve component, one load component, the config
    // entries the transform wire's manifest shape already defines
    // (`witWorld` rides the §15.1 kind-2 validation).
    std::fs::write(
        root.join("virtual.wasm"),
        resolve_component_wat("../virtual/shim.ts"),
    )
    .expect("write resolve component");
    std::fs::write(
        root.join("loader.wasm"),
        load_component_wat("virtual-module-source"),
    )
    .expect("write load component");
    std::fs::write(
        root.join("oxide.config.json"),
        concat!(
            r#"{"framework":{"adapter":"react"},"plugins":["#,
            r#"{"kind":"wasm","path":"./virtual.wasm","name":"resolve","#,
            r#""witWorld":"oxide:plugin/hooks@0.1.0","hooks":["resolveId"]},"#,
            r#"{"kind":"wasm","path":"./loader.wasm","name":"load","#,
            r#""witWorld":"oxide:plugin/hooks@0.1.0","hooks":["load"]}]}"#,
        ),
    )
    .expect("write config");

    // The wire: the SAME builder the build path (build/plan.rs) and the
    // dev client bundle call before handing `BundleOptions` to the
    // bundler. No plugins → `(None, None)`, the pre-E05 shape.
    let cfg = oxide_fw_config::load(&root, "development", BTreeMap::new())
        .expect("config loads")
        .config;
    let (resolve, load) = build_wasm_load_resolve_hooks(&root, &cfg);
    let resolve = resolve.expect("the resolveId-declaring component wires the resolve arm");
    let load = load.expect("the load-declaring component wires the load arm");

    let from = root.join("src/routes/page.tsx");
    println!("== resolve arm ==");
    println!(
        "  './missing.ts'        -> {:?}",
        resolve
            .call("./missing.ts", from.to_string_lossy().as_ref())
            .unwrap()
    );
    println!(
        "  ''  (decline)         -> {:?}  (the default resolver wins)",
        resolve.call("", from.to_string_lossy().as_ref()).unwrap()
    );
    println!("== load arm ==");
    println!(
        "  'shim.ts'             -> {:?}",
        load.call("shim.ts").unwrap()
    );
    println!(
        "  ''    (decline)       -> {:?}  (the disk read wins)",
        load.call("").unwrap()
    );
    println!("== the chain ==");
    println!("  build: rs arm -> js bridge -> wasm host (first non-declining answer wins)");
    println!("  dev:   js bridge -> wasm host");
    let _ = Path::new(&root); // the root stays the walkthrough's project
}
