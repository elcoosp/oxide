# Handoff — BUG-MAPKEY + BUG-SOLID-CLIENT fixed; every gate green

## Where things stand

**Branch:** `main`
**Working tree:** committed (see the latest `fix:` commits).
**Binary:** `cargo build -p oxide-fw-cli` (dev profile, `CARGO_PROFILE_DEV_DEBUG=0`).

The React-only HMR plan and the multi-framework HMR rollout (previous
handoffs) are intact. This session closed out the last red gates: the
production-build fixture gate (smoke-app-{vue,svelte,solid}) and the Solid
browser HMR suite, plus the create-oxide environment-dependent test.

## What landed this session

### 1. BUG-MAPKEY — raw SFCs reached Rolldown's parser (prod builds, vue/svelte/solid)

Symptom: `oxide build` died with the file-less
`[BLD-ROLLDOWN] rolldown write: Unexpected JSX expression` on every
non-React fixture (solid additionally: `"jsx" is not exported by
"@solidjs/web"`).

**The controlled experiment** (minifier hypothesis, forced minify off in the
backend via a temporary env override): the failure persisted with minify
off — hypothesis DISPROVEN, experiment override removed after.

Actual root cause, found by graph bisection + in-browser probes:
`resolve_relative` (`crates/cli/src/precompile.rs`) produced precompiled-map
keys with `..` segments (`/proj/src/routes/index/../../components/a.vue`)
while Rolldown resolved the same import to the normalized spelling. The
`PrecompiledPlugin` lookup missed, the RAW `.vue`/`.svelte` file flowed into
the parser (a `<template>` block parses as JSX), and Solid's raw `.tsx`
took the §6.3 automatic-runtime transform against `@solidjs/web` (no
`jsx`/`jsxs` exports there).

Fix (three layers):
- `resolve_relative` normalizes every returned path (the map-key contract).
- `PrecompiledPlugin::new` / `OxidePassesPlugin::new` normalize keys;
  `lookup` also tries the normalized id.
- NEW guard: `OxidePassesPlugin` gained a `load` hook — a raw `.vue` /
  `.svelte` / `.mdx` id with no precompiled content now fails the build
  with a NAMED `BLD-SFC` diagnostic (the old failure was undiagnosable).

### 2. BUG-SOLID-CLIENT — the browser silently painted the IR fallback (Atlas)

Symptom: `hmr-solid` browser tests got `h1 = "Hello Atlas"` (the adapter's
built-in §22 reference tree) while the server sent `<h1>Home</h1>`, with NO
console errors and NO failed requests.

Root cause: `packages/solid/src/{real-client,hydrate-island-real}.ts`
loaded the Solid web runtime via a VARIABLE-specifier `import(name)`.
Neither pipeline can rewrite a dynamic variable import: the dev prebundle
rewrites static literals to `/@prebundle/*.js`, and Rolldown resolves
literals from node_modules. The browser rejected the bare specifier with
`Failed to resolve module specifier` BEFORE any request; the catch turned
that into `undefined` → `mountSolidPageRoot` → silent IR fallback.
Vue/Svelte never hit this because their loaders use string literals.

Fix: `@solidjs/web` is now a static literal (rewritable + resolvable); the
Solid-1 `solid-js/web` fallback is assembled at runtime with
`['solid-js', 'web'].join('/')` — deliberate, because (a) as a literal
Rolldown fails every prod build with `Could not resolve 'solid-js/web'`
(Solid 2 has no `./web` export) and (b) esbuild constant-folds `'a' + 'b'`
back into the banned literal. `./real-ssr.ts` keeps its variable form
(Node resolves bare specifiers from real node_modules).

### 3. Console-noise fix: react-refresh runtime probe

The HMR runtime imported `/@prebundle/react-refresh-runtime.js` on EVERY
ESM-dev page. Non-React projects don't prebundle it → guaranteed 404 →
Chromium console error → the per-framework specs' error-free-console
assertion failed. `packages/core/src/client/hot.ts` now fetch-probes the
URL first (a 404 fetch is silent) and imports only on `ok`; a miss throws
into the existing `.catch` so `$RefreshRuntime$` stays unset and the
full-reload fallback semantics are unchanged.

### 4. BUG-PWD — create-oxide's workspace detection was environment-dependent

`packageJson()` chose `workspace:*` vs `0.1.0` via
`process.env['PWD'].includes('packages/')`. `PWD` doesn't track
`process.chdir()`, so `pnpm test:packages` from the repo root failed the
scaffold e2e while `pnpm -C packages/create-oxide test` passed. Fixed:
`isInsidePnpmWorkspace(dir)` walks up from the OUTPUT directory looking for
`pnpm-workspace.yaml`; `scaffold()` passes it explicitly; `packageJson()`
defaults to published mode. Tests updated + two new deterministic tests
(workspace layout pins `workspace:*`; walk-up detection).

### Test-pyramid additions this session

- `crates/bundler`: 9 unit tests (lookup normalization × 3, BLD-SFC guard × 6).
- `crates/cli`: 4 unit tests pinning `resolve_relative` normalization.
- `packages/create-oxide`: workspace-detection tests (+ fixed expectations).
- The e2e gates themselves are the integration/e2e regression net:
  `tests/fixtures/check.sh` (imported component SFCs through prod builds for
  all four frameworks), `tests/fixtures/plugins/check.sh` (7 cases incl.
  virtual modules + generateBundle), and `hmr-solid.spec.ts` (pins the
  client-loader contract end to end).

## Verified test state (this sandbox, chromium, `--workers=1 --retries=0`)

- **Gates**: `tests/fixtures/check.sh` 4/4 (react+vue+svelte+solid),
  `tests/fixtures/plugins/check.sh` 7/7.
- **Rust**: cli lib 257 (+MDX ignored test green via `--ignored`), bundler
  lib 27, assets 125, schema 4+6.
- **Node**: `pnpm test:packages` 598/598 (repo root — the PWD flake is
  dead); typecheck clean for core/create-oxide/vue/svelte/solid.
- **Playwright**: smoke+island 7, react HMR group 26+6, Group B 5,
  Group C 17, `hmr-vue` 7, `hmr-solid` 7, `hmr-svelte` 7 — all green.
- **Not runnable in this environment** (pre-existing, unchanged):
  webkit (missing GTK4/GStreamer), `sw.spec` ×2 (service-worker precache
  empty in headless chromium here).

## Sandbox notes for the next session

All previous HANDOFF gotchas still apply (60s beforeAll budgets, serial
gate, pristine-fixture syncing, `pkill` by PID, core dist rebuild after
`hot.ts`/`boot.ts` edits, staleness-scan scope, disk pressure dedupe,
`nasm` on PATH for wasmtime). Two additions:

1. **Adapter dist rebuild after touching `packages/*/src`**: the dev
   prebundle and the prod build both consume `packages/*/dist`. `pnpm -C
   packages/<fw> build` (tsdown) — then clear
   `examples/smoke-app-*/.oxide/esm-prebundle` so stale prebundles don't
   mask the change.
2. **Specifiers in adapter CLIENT code must be string literals** (rewritten
   by the dev prebundle, resolved by the prod bundler); a variable-specifier
   import silently kills the browser path. If a specifier must stay
   un-resolvable by Rolldown (optional peers that don't exist under the
   target runtime), assemble it at runtime with `['a','b'].join('/')` —
   NOT `'a' + 'b'` (esbuild constant-folds concatenation back into the
   literal that breaks the prod build).

## Where to pick up, in one sentence

Every gate is green; next work is publishing `solid-jsx-oxc` linux binaries
(the babel fallback stays the portable default), the deferred §3.8b browser
ESM-cache proxy, and keeping the suite green in CI (webkit needs a normal
machine).
