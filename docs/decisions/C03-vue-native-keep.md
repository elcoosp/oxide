# 2026-11-14

## C03 — Vue native compiler: SPIKED-KEEP, LANDED (WP V2-C03)

**Dependency arm (§0b).** `verter_core 0.0.1-beta.1`, **crates.io
latest** (published 2026-03-16; the upstream artifact of the verter
project). The reported alternative family (`vize_atelier_*`) does not
exist on crates.io — rejected for absence. No git pin needed: every
API the backend uses (the `compile` entry point,
`CodegenOptions::{filename, inline, target}`,
`VerterCompileOptions::{ssr, force_js}`) is present in the published
crate.

**Pin detail.** The published 0.0.1-beta line is internally skewed:
`verter_core` beta.1 compiles against `oxc_span` 0.117 while its
sibling `verter_span ^0.0.1-beta.1` resolves to beta.6 (built against
oxc_span 0.151) — 101 type-mismatch errors in the published pair.
The lockfile pins `verter_span --precise 0.0.1-beta.1`, which unifies
oxc_span 0.117 across both. The workspace's own oxc 0.155 stack is
untouched: the seam is string-in/string-out and the backend allocates
through `oxc_allocator` 0.117 (feature-gated alias dep).

**Spike pins (`crates/compiler/tests/vue_native_spike.rs`, all green).**
- API shape: `compile(input, &CodegenOptions, &VerterCompileOptions,
  &allocator) -> VerterCompileResult { script, template, styles,
  scope_id }`.
- TS handling: `<script setup lang="ts">` is lowered by the crate
  itself (`force_js`) — no second strip pass.
- Scope-id stability: `data-v-be3702e3` for the same filename across
  client, SSR and repeat runs (the acceptance contract).
- Module imports ride the `vue` runtime names the bundler prebundles.

**Worker-oracle parity.** `tooling:vue-compile` (the oracle): parse →
`compileScript(descriptor, { id, inlineTemplate: true, templateOptions:
{ ssr } })` → sucrase type strip. The backend mirrors it: `inline:
Some(!dev)` (the `inlineTemplate` shape), `VerterCompileOptions::ssr`
mapping `Generate::Ssr`, `force_js` (the sucrase strip), filename-driven
ids, and the worker's drop-styles behavior on this path (`css: None`).

**Landings.** `crates/compiler/src/native/vue.rs` (`VueNative`,
`vue-native@0.0.1-beta.1`, handles `vue`), the `native-vue` feature
(forwarded `oxide-fw-cli/native-vue`), the `native_compile/vue`
criterion group, the registry push in
`NativeRegistry::from_enabled_features`, and the
`scripts/compiler-equiv.sh vue` acceptance gate.
