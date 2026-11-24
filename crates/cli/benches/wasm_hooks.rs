//! §V2-E05: the WASM `load`/`resolveId` wire's bench — the per-module cost
//! a `kind: wasm` plugin adds once the CLI composes the host's
//! `call_resolve_hook`/`call_load_hook` into the bundler's
//! `BundleOptions.load_hook`/`resolve_hook` (the recorded remaining half of
//! the E05 row). The arms cover the shapes a build actually pays: the
//! answering round trip (a rewritten specifier, a virtual module's
//! source), the declining round trip (the `none` the canonical ABI
//! flattens, the pass-through the default resolver / disk read follows),
//! and the first-wins chain overhead with a second plugin riding the same
//! closure.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! wasm_hooks` (the bench-util opt-in keeps plain `cargo test` off
//! criterion); the V8-free build suffices — the bench never touches the
//! worker pool.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use oxide_fw_cli::build_wasm_load_resolve_hooks;

/// A component-model WAT exporting `resolve-id`: a non-empty specifier
/// resolves to `marker`, an empty one declines. The marker's byte length
/// rides the option header the canonical ABI flattens (the compat_tdd
/// fixture shape).
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

/// The matching `load` component: a non-empty specifier serves `marker` as
/// the virtual module's source; an empty one declines.
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

/// A temp project whose config declares the plugin set: every entry is the
/// `kind: wasm` shape the transform wire already parses, with `witWorld`
/// set (the manifest validation's PLG-WIT gate).
fn wire_project(
    markers: &[(&str, &str)],
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    oxide_fw_config::OxideConfig,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let mut plugins: Vec<String> = Vec::new();
    for (name, kind_marker) in markers {
        let file = format!("{name}.wasm");
        let wat = match *kind_marker {
            "resolve" => resolve_component_wat(&format!("../virtual/{name}.ts")),
            _ => load_component_wat("virtual-module-source"),
        };
        std::fs::write(root.join(&file), wat).expect("write component");
        let hooks = if *kind_marker == "resolve" {
            "resolveId"
        } else {
            "load"
        };
        plugins.push(format!(
            r#"{{"kind":"wasm","path":"./{file}","name":"{name}","witWorld":"oxide:plugin/hooks@0.1.0","hooks":["{hooks}"]}}"#
        ));
    }
    std::fs::write(
        root.join("oxide.config.json"),
        format!(
            "{{\"framework\":{{\"adapter\":\"react\"}},\"plugins\":[{}]}}\n",
            plugins.join(",")
        ),
    )
    .expect("write config");
    let cfg = oxide_fw_config::load(&root, "development", std::collections::BTreeMap::new())
        .expect("config loads")
        .config;
    (dir, root, cfg)
}

/// The answering + declining round trips through ONE composed resolve
/// hook, and the same through the load hook — the per-module cost the
/// wire adds to a build that declares a WASM plugin.
fn bench_wasm_hook_round_trips(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasm_plugin_hooks");
    let (_dir, root, cfg) = wire_project(&[("resolve", "resolve"), ("loader", "load")]);
    let (resolve, load) = build_wasm_load_resolve_hooks(&root, &cfg);
    let resolve = resolve.expect("resolve arm wired");
    let load = load.expect("load arm wired");
    let from = "/proj/src/routes/page.tsx";
    group.bench_function("resolve_answer", |b| {
        b.iter(|| {
            black_box(
                resolve
                    .call(black_box("./missing.ts"), black_box(from))
                    .unwrap(),
            )
        });
    });
    group.bench_function("resolve_decline", |b| {
        b.iter(|| black_box(resolve.call(black_box(""), black_box(from)).unwrap()));
    });
    group.bench_function("load_answer", |b| {
        b.iter(|| black_box(load.call(black_box("/proj/src/routes/shim.ts")).unwrap()));
    });
    group.bench_function("load_decline", |b| {
        b.iter(|| black_box(load.call(black_box("")).unwrap()));
    });
    group.finish();
}

/// The chain overhead: TWO resolve components ride one closure and the
/// first non-declining answer wins — the per-module cost when the project
/// declares two resolving plugins (one decline + one answer per call).
fn bench_wasm_hook_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("wasm_plugin_hooks_chain");
    let (_dir, root, cfg) = wire_project(&[("a", "resolve"), ("b", "resolve")]);
    let (resolve, _load) = build_wasm_load_resolve_hooks(&root, &cfg);
    let resolve = resolve.expect("resolve arm wired");
    let from = "/proj/src/routes/page.tsx";
    group.bench_function("two_plugins_first_wins", |b| {
        b.iter(|| {
            black_box(
                resolve
                    .call(black_box("./missing.ts"), black_box(from))
                    .unwrap(),
            )
        });
    });
    group.finish();
}

criterion_group!(benches, bench_wasm_hook_round_trips, bench_wasm_hook_chain,);
criterion_main!(benches);
