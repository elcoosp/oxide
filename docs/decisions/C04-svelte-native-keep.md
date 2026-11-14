# 2026-11-14

## C04 — Svelte native compiler: SPIKED-KEEP, LANDED (WP V2-C04)

**Dependency arm (§0b).** `svelte-compiler 0.1.4`, **crates.io latest**
(published 2026-03-16 — ~8 months old, but it IS the upstream artifact:
no newer unreleased API exists that the backend needs, so §0b(2)'s
git-rev arm does not apply). The spec's claimed API is present verbatim:
`compile`, `compile_module`, `parse`, `print`, `preprocess`, `migrate`.

**Spike pins (`crates/compiler/tests/svelte_native_spike.rs`, all green).**
- Svelte 5 runes (`$state`, `$derived`, `$props`) compile for both
  targets — none of the kill-criterion runes is rejected.
- `GenerateTarget::Client | Server` maps onto the worker's
  `generate: 'dom' | 'ssr'`.
- `CssOutputMode::Injected` matches the worker's `css: 'injected'`
  option (style rides the emitted JS; no separate artifact).
- The `hmr` + `dev` options compile; warning text passes through.

**Worker-oracle parity.** `tooling:svelte-compile` passes
`{ filename, generate: 'dom'|'ssr', css: 'injected' }` and returns
`js.code`; the backend passes the exact same three options onto
`CompileOptions` and emits `result.js.code` (the crate lowers TS
scripts itself, so the worker's best-effort sucrase strip has nothing
left to do on the native path).

**Landings.** `crates/compiler/src/native/svelte.rs` (`SvelteNative`,
`svelte-native@0.1.4`, handles `svelte`), the `native-svelte` feature
(forwarded `oxide-fw-cli/native-svelte`), the `native_compile/svelte`
criterion group, the registry push, and
`scripts/compiler-equiv.sh svelte`.
