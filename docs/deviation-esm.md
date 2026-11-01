# Deviation: Native ESM dev server (Phase 4)

Status: **active**, `dev.esm` defaults to `true` since commit
`feat(dev): React Refresh AST transform + runtime hookup (Phase 3)`.

## What changed from the Rolldown-bundle default

`oxide dev` now serves `.tsx`/`.ts`/`.jsx`/`.js` sources **as transformed
ESM**, one request per module, through a per-file Oxc pass (~8ms warm). There
is no client bundle emitted by the dev server (though one is still produced
at cold start to keep CSS working — see the "CSS gap" section).

The bundler path remains fully supported; a project that sets
`dev: { esm: false }` gets the previous behaviour byte-for-byte.

## URL families (all served by `crates/cli/src/esm_dev.rs`)

| Prefix | Serves |
|---|---|
| `/src/**/*.tsx` | Raw source, lowered to ESM with `$RefreshReg$` injections and bare-specifier rewrites |
| `/@prebundle/<name>.js` | Prebundled CJS deps (react, react-dom, react-refresh/runtime) and copied `packages/core/dist/` |
| `/@boot/boot.mjs` | Boot entry: imports `@oxide-fw/core/client/hot` first, then each route component |
| `/@esm-error/<encoded>` | Throwing module: Vite's pattern for a failed dynamic import |

## Import rewriting

No importmap. Each `import x from 'react'` is rewritten in-place to
`from '/@prebundle/react.js'` using `oxide_fw_compiler::scan_import_spans`
(a real tokenizer, so strings and comments are skipped). Relative imports
(`./foo.js`) pass through unchanged.

## HMR

- **`HmrMessage::ModuleUpdate`** gained an optional `url` field (Phase 2).
  The watcher's `AdapterHmr` branch publishes `{ path, url: "/src/…" }` for
  a `src/**` change. Non-`src/**` changes publish `FullReload`.
- **`hot.ts`** installs `globalThis.__oxide_hmr__` on first import and
  routes `reloadModule(url)` through `$RefreshRuntime$.performReactRefresh()`
  when available, else `location.reload()`.
- **`hmr.ts`**'s `module-update` case reads `msg.url` and calls
  `__oxide_hmr__.reloadModule(url)`; if `url` is absent (old message), it
  falls back to `location.reload()`.

## React Refresh injection (Phase 3)

`oxide_fw_compiler::lower_to_js_with_refresh` lowers JSX then walks the
**lowered** AST for top-level `function`, `class`, and
`const/let/var = <arrow|function|class>` bindings. Each is wrapped in
`$RefreshReg$(Name, "Name")`. The shim is:

```js
const $RefreshReg$ = (t, id) => {
  const r = globalThis.__oxide_refresh_reg;
  if (typeof r === 'function') { try { r(t, id, import.meta.url); } catch {} }
};
const $RefreshSig$ = () => (t) => t;

The transform is **idempotent** (sources that already contain
`$RefreshReg$` are returned unchanged) and **skips** non-tsx/jsx sources.

## Route adds

`RestartSubsystems` in the watcher's route-file branch:
- In bundler mode: rebuilds the client bundle (existing behaviour).
- In ESM mode: regenerates `.oxide/esm-manifest.json` and
  `.oxide/esm-boot.mjs` from the new tree; the running `EsmDevState` is
  not rebuilt — the rewrite map depends only on the framework exports and
  the prebundle list, which do not change when a route is added.

Either way it publishes a `FullReload` — the boot entry has a new set of
imports, and the client has to re-fetch it.

## Cold start (Phase 5 — closed)

**Phase 5 skips `ensure_dev_client_bundle` in ESM mode.** The boot entry
imports source modules directly and nothing in the browser fetches a
bundled chunk, so the Rolldown pass produced artifacts nothing consumed.

CSS continues to work through two paths that run on every cold start
regardless of the bundle:

1. **`tailwind::precompute`** writes `dist/client/tailwind-<hash>.css` for
   projects with a Tailwind plugin (the website). It uses the tooling pool
   + LightningCSS, not Rolldown.
2. **`AdapterPageRenderer::inject_route_css`**'s disk-scan fallback
   (wr101) emits `<link rel="stylesheet">` for any `tailwind-*.css` it
   finds in `dist/client/`. `with_manifest_path` is set unconditionally
   by `build_framework_stack`, so the scan fires even without a
   `client.manifest.json`.

Cold-start numbers are not asserted in CI (they depend on
`node_modules` cache state and tooling-pool warmup); the change removes
the Rolldown pass, which was the largest single contributor.

## CSS imports inside components (Phase 5b — closed)

A project that `import './styles.css'` inside a route component now works:

- **Serving.** The import is rewritten to `/@css/<src-relative>`, which
  returns a JS module that injects a `<style data-oxide-css="...">` on
  first evaluation and updates `textContent` in place on every re-import.
- **HMR.** A save on a `.css` file under `src/` publishes
  `ModuleUpdate { url: "/@css/<rel>" }`; `reloadModule` re-imports it with
  a cache-bust, the module body re-runs, and the `<style>` tag swaps — no
  page reload.
- **Tailwind entries.** A CSS file containing `@import "tailwindcss"` is
  served as an *empty* stub. The generated CSS reaches the browser
  through the SSR renderer's disk-scan `<link>` (`inject_route_css`), so
  there is exactly one source of truth and no duplicate `<style>`.
- **Escape guard.** A `../..` chain that would leave `src/` is rejected at
  rewrite time; the specifier is left untouched and the browser reports a
  clear 404.

### The reload contract

`reloadModule(url)` clears the module's accept callbacks, re-imports the
URL with `?t=<ms>-<seq>` (a monotonic counter after the millisecond —
`Date.now()` alone collides on back-to-back synchronous reloads, serving
the cached module and never re-running the fixture), then fires the
accept callbacks the **fresh** evaluation registered. A module must
re-register on every evaluation — the dev server's preamble (a
fixed-literal `createHotContext("/src/…")`) plus `inject_hmr_boundary`'s
appended `import.meta.hot.accept(...)` guarantees this. The pinned
contract lives in `packages/core/test/hot.test.ts`.

## Save timings (dev mode)

| Save | What fires | Approx latency |
|---|---|---|
| `.tsx` component | React Refresh re-import + `performReactRefresh()` | ~25ms, state preserved |
| `.css` in a route | `/@css/` module re-import, `<style>` swaps | ~15ms |
| `.css` with `@import "tailwindcss"` | tailwind precompute + `<link>` href bump | 200–1000ms (scanner + LightningCSS) |
| `.server.ts` | loader re-run + `ox:data` dispatch | 30–100ms |

The 60× improvement the plan projected holds for component edits — the
common case. Tailwind edits pay the Tailwind compiler cost, not an Oxide
cost.

## Behaviour changes users will notice

1. **First page load in `oxide dev` is faster** — a per-route ~30ms Oxc
   pass instead of a ~150ms bundle cache hit.
2. **Saves are ~60× faster on component edits** — 25ms with React Refresh
   instead of 1.5s with the Rolldown incremental pass.
3. **`<script type="module" src="/@boot/boot.mjs">`** appears in `<head>`
   instead of `/client/index-<hash>.js`.
4. **`/src/**/*.tsx` becomes a real HTTP URL** in dev. `.server.*` sources
   return 403 (never served to the browser).

## Rollback

Set `dev: { esm: false }` in `oxide.config.ts`. The Rolldown bundle path
takes over on the next `oxide dev` restart; nothing else in the project
needs to change. The `.oxide/esm-*` files are ignored when `dev.esm=false`.

## Test coverage and known gaps

### What `tests/playwright/tests/hmr.spec.ts` pins

Every regression hit while building the native ESM dev server is caught by
an assertion in that file. The mapping:

| Regression | Assertion |
|---|---|
| `//@boot/boot.mjs` doubled slash → `ERR_NAME_NOT_RESOLVED` | `boot script URL has exactly one leading slash` |
| Boot entry imports server-only routes (`route.ts`, `socket.ts`, `middleware.ts`, `*.server.*`) → 403 halts graph | `boot entry never imports server-only route files` |
| Islands imported as named (`{ __island_X }`) instead of default | `boot entry uses default-import for islands` |
| `bootWithRouter` / `clientAdapter` missing from boot → no hydration | `boot entry imports boot runtime + adapter + islands` |
| `componentFor` only reads `entry_chunk` → client falls back to IR | `ESM manifest: every entry has a component URL` |
| CJS IIFE-wrapped exports not extracted by Rolldown → `export const useState` missing | `prebundle: react.js has ≥10 named exports` |
| CJS wrapper (`module.exports = require('./cjs/…')`) hides exports from Rolldown | `prebundle: react-jsx-runtime.js has named JSX factories` |
| `external` ignored → React inlined 3× → "Invalid hook call" | `prebundle: react-dom-client.js externalizes react` |
| `require('react')` left as `__require` shim → "Calling require" | same |
| Lost first `$RefreshReg$` registration (runtime not yet loaded) | `prebundle: client-hot.js buffers refresh registrations` |
| `$RefreshReg$` id prefixed with `import.meta.url` → `?t=` makes every reload a new component | `prebundle: client-hot.js buffers refresh registrations` + `served page.tsx has stable $RefreshReg$ id` |
| Server-only imports (`import type { X } from './x.server.ts'`) leak into served ESM | `served page.tsx does not leak .server. imports` |
| Any of the above (or a new failure) preventing module-graph load | `page hydrates without module-load errors` (asserts the exact console patterns absent) |
| HMR WebSocket never opens | `HMR WebSocket connects to /_oxide/hmr` |
| Save doesn't update the DOM | `HMR: editing a component updates the DOM without reloading` |
| Save works once but triggers a full reload on later edits (registration drift) | `HMR: 3 consecutive edits each update the DOM, never reload` |

The "no reload" assertion is `performance.timeOrigin` — the epoch of the
document's navigation start. Any `location.reload()` resets it; HMR does
not. A `window`-attached session marker is the second, redundant signal.

### What is NOT covered (and why)

1. **Firefox and WebKit.** `playwright.config.ts` declares three projects,
   but the CI matrix runs them individually and only chromium is exercised
   in this session. A Gecko- or WebKit-only regression (e.g. a
   `performance.timeOrigin` semantics change, `import.meta.hot` inside a
   worker on Safari) would not be caught by the chromium-only run.

2. **Cold-start time bound.** The "IT'S FAST" claim (≈25 ms for a component
   save) is not asserted as a numeric bound — CI machines vary by
   10–100×. Playwright's default 15 s assertion timeout acts as a
   pathological-failure gate. A real timing regression (say 25 ms → 800 ms)
   would not trip the test. Asserting it requires a benchmark
   environment with fixed CPU and warm page cache that this repo does
   not have.

3. **`react-refresh` absent.** HMR falls back to `location.reload()` when
   the project does not install `react-refresh`. The test suite skips the
   HMR tests in that case (documented behaviour, not a bug), so a project
   that removes `react-refresh` won't fail CI — it silently loses HMR.

4. **CSS HMR.** The `/@css/**` module self-accept path is exercised by
   `packages/core/test/hot.test.ts` at the unit level, but no browser
   test edits a `.css` file and confirms a `<style>` tag updated in
   place. A project that imports CSS from a component and relies on the
   `<style>` swap is not verified end-to-end.

5. **`import.meta.hot` in workers.** The dev server installs
   `import.meta.hot` for the main thread only. Any HMR inside a `new
   Worker(...)` will see `import.meta.hot === undefined` and the
   `if (import.meta.hot) { … }` guard will silently no-op. Not a
   regression from this implementation, but worth naming.

6. **Multi-worker dev servers.** The HMR hub uses an in-process
   `tokio::sync::broadcast`. Two dev servers on the same project (a
   restart race) have separate hubs; a WebSocket connected to one will
   not see file changes from the other. `SO_REUSEPORT` zero-downtime
   reload is tested for the "old drains, new accepts" flow only.

7. **`oxide build` regression.** Every regression in this session was in
   `oxide dev`. The production build uses `RolldownBundler` and never
   touches `prebundle.rs`, `cjs_exports.rs`, or `esm_dev.rs`. There is
   no test asserting "the production build output is unchanged" — a
   future refactor of `prebundle.rs` that also touches a shared helper
   could theoretically affect `oxide build`.

8. **Playwright is not part of `cargo nextest run --workspace`.** The
   Rust suite (1213 tests) is the primary gate. Playwright
   (`pnpm --filter @oxide/playwright test`) is a separate manual step
   that needs `node_modules` + a built `oxide` binary. A CI pipeline
   that only runs `cargo nextest` will not catch a browser regression
   even though the tests exist.

## Test coverage and known gaps

### What `tests/playwright/tests/hmr.spec.ts` pins

Every regression hit while building the native ESM dev server is caught by
an assertion in that file. The mapping:

| Regression | Assertion |
|---|---|
| `//@boot/boot.mjs` doubled slash → `ERR_NAME_NOT_RESOLVED` | `boot script URL has exactly one leading slash` |
| Boot entry imports server-only routes (`route.ts`, `socket.ts`, `middleware.ts`, `*.server.*`) → 403 halts graph | `boot entry never imports server-only route files` |
| Islands imported as named (`{ __island_X }`) instead of default | `boot entry uses default-import for islands` |
| `bootWithRouter` / `clientAdapter` missing from boot → no hydration | `boot entry imports boot runtime + adapter + islands` |
| `componentFor` only reads `entry_chunk` → client falls back to IR | `ESM manifest: every entry has a component URL` |
| CJS IIFE-wrapped exports not extracted by Rolldown → `export const useState` missing | `prebundle: react.js has ≥10 named exports` |
| CJS wrapper (`module.exports = require('./cjs/…')`) hides exports from Rolldown | `prebundle: react-jsx-runtime.js has named JSX factories` |
| `external` ignored → React inlined 3× → "Invalid hook call" | `prebundle: react-dom-client.js externalizes react` |
| `require('react')` left as `__require` shim → "Calling require" | same |
| Lost first `$RefreshReg$` registration (runtime not yet loaded) | `prebundle: client-hot.js buffers refresh registrations` |
| `$RefreshReg$` id prefixed with `import.meta.url` → `?t=` makes every reload a new component | `prebundle: client-hot.js buffers refresh registrations` + `served page.tsx has stable $RefreshReg$ id` |
| Server-only imports (`import type { X } from './x.server.ts'`) leak into served ESM | `served page.tsx does not leak .server. imports` |
| Any of the above (or a new failure) preventing module-graph load | `page hydrates without module-load errors` (asserts the exact console patterns absent) |
| HMR WebSocket never opens | `HMR WebSocket connects to /_oxide/hmr` |
| Save doesn't update the DOM | `HMR: editing a component updates the DOM without reloading` |
| Save works once but triggers a full reload on later edits (registration drift) | `HMR: 3 consecutive edits each update the DOM, never reload` |

The "no reload" assertion is `performance.timeOrigin` — the epoch of the
document's navigation start. Any `location.reload()` resets it; HMR does
not. A `window`-attached session marker is the second, redundant signal.

### What is NOT covered (and why)

1. **Firefox and WebKit.** `playwright.config.ts` declares three projects,
   but the CI matrix runs them individually and only chromium is exercised
   in this session. A Gecko- or WebKit-only regression (e.g. a
   `performance.timeOrigin` semantics change, `import.meta.hot` inside a
   worker on Safari) would not be caught by the chromium-only run.

2. **Cold-start time bound.** The "IT'S FAST" claim (≈25 ms for a component
   save) is not asserted as a numeric bound — CI machines vary by
   10–100×. Playwright's default 15 s assertion timeout acts as a
   pathological-failure gate. A real timing regression (say 25 ms → 800 ms)
   would not trip the test. Asserting it requires a benchmark
   environment with fixed CPU and warm page cache that this repo does
   not have.

3. **`react-refresh` absent.** HMR falls back to `location.reload()` when
   the project does not install `react-refresh`. The test suite skips the
   HMR tests in that case (documented behaviour, not a bug), so a project
   that removes `react-refresh` won't fail CI — it silently loses HMR.

4. **CSS HMR.** The `/@css/**` module self-accept path is exercised by
   `packages/core/test/hot.test.ts` at the unit level, but no browser
   test edits a `.css` file and confirms a `<style>` tag updated in
   place. A project that imports CSS from a component and relies on the
   `<style>` swap is not verified end-to-end.

5. **`import.meta.hot` in workers.** The dev server installs
   `import.meta.hot` for the main thread only. Any HMR inside a `new
   Worker(...)` will see `import.meta.hot === undefined` and the
   `if (import.meta.hot) { … }` guard will silently no-op. Not a
   regression from this implementation, but worth naming.

6. **Multi-worker dev servers.** The HMR hub uses an in-process
   `tokio::sync::broadcast`. Two dev servers on the same project (a
   restart race) have separate hubs; a WebSocket connected to one will
   not see file changes from the other. `SO_REUSEPORT` zero-downtime
   reload is tested for the "old drains, new accepts" flow only.

7. **`oxide build` regression.** Every regression in this session was in
   `oxide dev`. The production build uses `RolldownBundler` and never
   touches `prebundle.rs`, `cjs_exports.rs`, or `esm_dev.rs`. There is
   no test asserting "the production build output is unchanged" — a
   future refactor of `prebundle.rs` that also touches a shared helper
   could theoretically affect `oxide build`.

8. **Playwright is not part of `cargo nextest run --workspace`.** The
   Rust suite (1213 tests) is the primary gate. Playwright
   (`pnpm --filter @oxide/playwright test`) is a separate manual step
   that needs `node_modules` + a built `oxide` binary. A CI pipeline
   that only runs `cargo nextest` will not catch a browser regression
   even though the tests exist.
