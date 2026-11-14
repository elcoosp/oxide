# 2026-11-14

## C02 — Solid native compiler: KILLED (WP V2-C02)

**Decision.** C02 is killed under the brief's §0b(4) arm 2: the crate is
**unpublished, unmirrored, and has no git repo a `rev` can pin**. No
dependency line, no `native-solid` feature flag, no stub file lands.

**The measured facts.**

1. crates.io: `solidjs-compiler` does not exist (sparse index
   `https://index.crates.io/so/li/solidjs-compiler` answers empty). A
   crates.io search for "solid compiler" returns only Solidity/EVM
   tooling (solang, solar-compiler, slang_solidity, ...) — nothing that
   compiles Solid-JSX.
2. Upstream: the `solidjs/solid` monorepo (cloned 2026-11-14,
   `git ls-tree -r HEAD --name-only`) contains **zero `Cargo.toml`
   files** — the research report's `packages/compiler` Rust crate (oxc
   0.144) does not exist at upstream `main` today.
3. No standalone mirror: `git ls-remote` probes for
   `solidjs/solid-compiler`, `solidjs/compiler`, `solidjs/solid-jsx`
   all answer "repository not found".
4. The worker's Solid path today uses `solid-jsx-oxc` (a NAPI binding)
   with `babel-preset-solid` fallback — there is no Rust crate behind
   either arm.

**Consequence.** Solid keeps the Node tooling pool
(`tooling:solid-compile` in `oxide-worker.mjs`) as its only compiler.
`framework.compilerBackend: 'native'` refuses Solid files with
`CMP-NAT-001` — the C01 contract holds. The spec's D27 vendoring arm
is void per the brief (§0a), so the monorepo-subdirectory extraction
the spec sketches is not performed either.

**Re-run conditions.** If Solid ships a Rust compiler to crates.io, or
a standalone repo a full-SHA `rev` can pin, re-open the WP and re-run
the spike protocol (worker oracle: `moduleName: "@solidjs/web"`,
`generate: 'dom'|'ssr'`, `hydratable: true`, filename; sucrase type
strip; the `solid-js/web` → `@solidjs/web` import rewrite).
