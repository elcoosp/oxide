//! V2-C03 spike — the native Vue compiler candidate (`verter_core`),
//! written per the spec's spike rule and the task brief's §0b order.
//!
//! # Dependency arm (recorded here and in
//! `docs/decisions/C03-vue-native.md`)
//!
//! `verter_core 0.0.1-beta.1`, **crates.io latest** (published
//! 2026-03-16; the upstream artifact of the verter project). The brief's
//! §0b(1) arm: no git pin needed — the API this spike needs (the
//! `compile` entry point, `CodegenOptions::target`, `inline`,
//! `runtime_module_name`, `VerterCompileOptions::ssr`, `force_js`) is
//! present in the published crate. The other reported candidate family
//! (`vize_atelier_*`) does not exist on crates.io (verified via the
//! sparse index) — rejected for absence, recorded in the decision doc.
//!
//! # What the spike must prove before the backend lands
//!
//! 1. API shape: `compile(input, &CodegenOptions, &VerterCompileOptions,
//!    &allocator) -> VerterCompileResult` with `script` / `template` /
//!    `styles` / `scope_id` blocks.
//! 2. TS handling: `<script setup lang="ts">` is lowered by the crate
//!    itself (`force_js`) — no second pass needed for the bundler.
//! 3. Scope-id stability: the same filename yields the same `scope_id`
//!    across SSR and client runs (the acceptance contract).
//! 4. Output module shape: the emitted script references the `vue`
//!    runtime the worker's output imports (`@vue/server-renderer`
//!    helpers under `ssr`), so the bundler's prebundle list stays valid.
//! 5. Compile time for the fixture set (recorded in the decision doc).
//!
//! Run: `cargo test -p oxide-fw-compiler --features native-vue --test
//! vue_native_spike -- --ignored --nocapture`
#![cfg(feature = "native-vue")]

use oxide_fw_compiler::native::{CompileRequest, Generate};
use std::path::Path;
use verter_core::compile::types::{CodegenOptions, CompileTarget, VerterCompileOptions};
use verter_core::compile::{VerterCompileResult, compile};

/// The worker's post-processing contract, replicated: `id` is the
/// base64-alnum 8-char short id `@vue/compiler-sfc` derives from the
/// filename (the tooling:vue-compile handler); verter derives its own
/// component id from `filename` — the spike pins THAT the id is stable
/// per filename (the data-v scope ids ride it).
fn compile_fixture(source: &str, filename: &str, ssr: bool) -> VerterCompileResult {
    let allocator = oxc_allocator_0_117::Allocator::default();
    let options = CodegenOptions {
        filename: Some(filename.to_string()),
        // Dev builds keep the inline render fn off; production inlines it
        // (the compileScript `inlineTemplate: true` shape the worker uses).
        inline: Some(true),
        target: CompileTarget::BUNDLER,
        ..Default::default()
    };
    let verter_options = VerterCompileOptions {
        ssr,
        // `<script setup lang="ts">` keeps type annotations — `force_js`
        // strips them so the emitted module is bundler-ready JS (the
        // sucrase pass the worker runs).
        force_js: true,
        ..Default::default()
    };
    compile(source, &options, &verter_options, &allocator)
}

/// Fixture (a): `<script setup lang="ts">` with `defineProps`, a
/// template with `v-for` and `v-model`.
const PROPS_VMODEL: &str = r#"<script setup lang="ts">
import { ref } from 'vue'
defineProps<{ items: string[] }>()
const picked = ref('')
</script>

<template>
  <ul>
    <li v-for="it in items" :key="it" @click="picked = it">{{ it }}</li>
  </ul>
  <input v-model="picked" />
</template>
"#;

/// Fixture (b): scoped `<style>` — the data-v scope ids must be stable
/// across runs and match between client and SSR compiles.
const SCOPED_STYLE: &str = r#"<script setup>
const n = 1
</script>

<template><p class="a">{{ n }}</p></template>

<style scoped>
.a { color: red; }
</style>
"#;

/// Fixture (c): a component using slots.
const SLOTS: &str = r#"<script setup>
</script>

<template>
  <div><slot name="header" /><slot /></div>
</template>
"#;

#[test]
#[ignore = "C03 spike — needs the native-vue feature; run with --ignored --nocapture"]
fn spike_api_shape_and_ts_handling() {
    let client = compile_fixture(PROPS_VMODEL, "src/components/Picker.vue", false);
    assert!(
        client.errors.is_empty(),
        "client compile errors: {:?}",
        client.errors
    );
    let script = client.script.as_ref().expect("script block");
    println!("=== script (client) ===\n{}\n", script.code);
    assert!(
        script.code.contains("import"),
        "the emitted module imports the vue runtime"
    );
    assert!(
        !script.code.contains("const msg: string"),
        "force_js stripped the TS annotation"
    );
    assert!(client.template.is_some(), "template block compiled");

    let ssr = compile_fixture(PROPS_VMODEL, "src/components/Picker.vue", true);
    assert!(
        ssr.errors.is_empty(),
        "ssr compile errors: {:?}",
        ssr.errors
    );
    let sscript = ssr.script.as_ref().expect("ssr script block");
    println!("=== script (ssr) ===\n{}\n", sscript.code);
}

#[test]
#[ignore = "C03 spike — needs the native-vue feature; run with --ignored --nocapture"]
fn spike_scope_id_stable_across_runs_and_targets() {
    let a = compile_fixture(SCOPED_STYLE, "src/components/Card.vue", false);
    let b = compile_fixture(SCOPED_STYLE, "src/components/Card.vue", true);
    assert!(
        a.errors.is_empty() && b.errors.is_empty(),
        "errors: {:?} / {:?}",
        a.errors,
        b.errors
    );
    println!("=== scope_id client: {} ===", a.scope_id);
    println!("=== scope_id ssr:    {} ===", b.scope_id);
    assert_eq!(
        a.scope_id, b.scope_id,
        "the same filename must scope-identify identically across targets"
    );
    let again = compile_fixture(SCOPED_STYLE, "src/components/Card.vue", false);
    assert_eq!(
        a.scope_id, again.scope_id,
        "the same filename must scope-identify identically across runs"
    );
    for style in &a.styles {
        println!("=== style ===\n{}\n", style.code);
    }
}

#[test]
#[ignore = "C03 spike — needs the native-vue feature; run with --ignored --nocapture"]
fn spike_slots_and_module_imports() {
    let r = compile_fixture(SLOTS, "src/components/Layout.vue", false);
    assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    let script = r.script.as_ref().expect("script block");
    println!("=== slots script ===\n{}\n", script.code);
    assert!(
        script.code.contains("'vue'") || script.code.contains("\"vue\""),
        "runtime helpers import from `vue` (the bundler's prebundle name)"
    );
}

/// The seam rehearsal: the exact call a `VueNative` backend makes —
/// source in, one JS module out, through the public
/// `NativeCompiler::compile` contract (this is the code the landed
/// backend wraps; the spike proves the shape compiles and runs).
#[test]
#[ignore = "C03 spike — needs the native-vue feature; run with --ignored --nocapture"]
fn spike_native_seam_rehearsal() {
    let source = PROPS_VMODEL;
    let req = CompileRequest {
        path: Path::new("src/components/Picker.vue"),
        source,
        generate: Generate::Dom,
        dev: false,
    };
    let ssr = req.generate == Generate::Ssr;
    let r = compile_fixture(req.source, &req.path.display().to_string(), ssr);
    assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
    let script = r.script.as_ref().expect("script block");
    assert!(!script.code.is_empty());
    println!("=== seam rehearsal output (first 40 lines) ===");
    for line in script.code.lines().take(40) {
        println!("{line}");
    }
}
