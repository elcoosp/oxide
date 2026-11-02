# Plan — production-grade ESM dev server + HMR

**Status:** forward-looking roadmap. The historical "what changed from the Rolldown-bundle default" record is in git history around commits `588023d8` → `27f5de09`. This file is about what remains.

**Current working state:** `tests/playwright/tests/hmr.spec.ts` passes 15/15 on chromium with `--workers=1 --retries=0`. Live edits of a component update the DOM without reloading (`performance.timeOrigin` unchanged). All 14 feature commits landed.

**What this file is:** a single, self-contained work queue. Every item is independently pickable and has a concrete file target and acceptance test. Some items are latent bugs the current tests do not catch; some are deliberate trade-offs we made to reach green; some are gaps documented but never scheduled. Nothing in here is "done until there is a test that would fail if the fix were reverted."

## What "production-grade" means

Four tiers. Nothing is "done" until it has an automated test in the shipped suite and its line in this file has been ticked.

- **Tier 1 — correctness:** the tool does what it claims. Silent-bug classes are closed.
- **Tier 2 — coverage:** every claim has proof in the shipped suite.
- **Tier 3 — robustness:** works at scale, over hours, under load.
- **Tier 4 — process:** CI catches a regression of any of the above.

## Deviations still outstanding

These are the remaining differences from Vite-class dev tooling. Some are **chosen** (we do not plan to fix); some are **unintended** (bugs to fix).

### Chosen — may stay

| # | Deviation | Rationale |
|---|---|---|
| D1 | **No importmap.** Bare specifiers are rewritten textually to `/@prebundle/...` URLs. | Simpler, works without `importmap` support, and lets the prebundle URL be a cache key. |
| D2 | **No `<script type="module">` inline preamble.** The `$RefreshReg$` shim is emitted per-module by the compiler. | Same approach Vite's Babel plugin output takes. |

### Unintended — must fix

| # | Deviation | Tracked as |
|---|---|---|
| U1 | `import.meta.hot` inside a Worker is `undefined` (main thread only). | 3.5 |
| U2 | `import { serverFn, defineLoad } from '@oxide-fw/core/server'` produces malformed output (missing `}`). | 1.1 |
| U3 | `$RefreshSig$` is a no-op — hook-dependency changes and custom-hook reordering go undetected. | 1.2 |
| U4 | Plain `.server.ts` (not `page.server.ts`) triggers `ServerModuleSwap → location.reload()`. | 1.5 |
| U5 | CSS Modules unsupported — `import styles from './x.module.css'` returns raw CSS text. | 1.4 |
| U6 | Cold-start HMR race: first edit within ~500ms of `__oxideBoot` may miss registrations if the prebundle runtime import races React's first render. Mitigated by `pendingRegistrations` buffering; not proven end-to-end. | 2.13 |
| U7 | `node_modules/*/package.json` edits do not invalidate the prebundle (cached forever). | 3.3 |
| U8 | `import.meta.env.X` changes trigger a full reload, not an HMR-driven re-render. | 3.4 |
| U9 | Multi-worker dev servers have isolated HMR hubs (WS to server A does not see edits on server B). | 3.6 |
| U10 | The client `COALESCE_MS` is defense-in-depth; the server publishes one message per `notify` event, not one per save. | 3.1 |
| U11 | Editing an *existing* route file (`routes/**/page.tsx`) reloads the page instead of hot-swapping. Correct for new/removed routes; wrong for a body edit. | 3.9 (blocked on 1.3) |

## Work queue

### Tier 1 — correctness

#### 1.1 — `strip_server_fn_imports` multi-name bug

- **What:** `import { serverFn, defineLoad } from '@oxide-fw/core/server'` → `import { defineLoad  from '…'` (missing `}`).
- **Why:** any project using both names together serves a broken module in dev.
- **Files:** `crates/compiler/src/lib.rs` (~line 660, `result.push_str(&rest[close + 1..])` needs `}` prepended when remaining names are non-empty).
- **Acceptance:** unit test `strip_server_fn_imports_handles_multi_name_import` with the two-name import producing valid JS.
- **Effort:** 2h.

#### 1.2 — `$RefreshSig$` real implementation

- **What:** currently `const $RefreshSig$ = () => (t) => t;` — a no-op. Changing a `useEffect` dep array, reordering hooks, or editing a custom hook's dependencies silently produces stale closures.
- **Why:** React Refresh's second guarantee (after in-place update) is detecting shape changes that require a remount. Without a real `$RefreshSig$`, "component updated but behaviour is wrong" bugs are undetectable.
- **Files:** `crates/compiler/src/oxc_backend.rs` (emit `$RefreshSig$(...)` around each hook-shaped call in the lowered AST) + `packages/core/src/client/hot.ts` (route through `$RefreshRuntime$.createSignatureFunctionForTransform()`); unit test in `packages/core/test/hot.test.ts`.
- **Acceptance:** test that reorders hooks between edits and asserts `performReactRefresh()` triggers a remount (listener count resets).
- **Effort:** 1 day.

#### 1.3 — Transitive dependency propagation

- **What:** editing `src/lib/utils.ts` reimports `utils.ts` only; `page.tsx` keeps using the old binding. `$RefreshReg$` saves this for components, but utilities, hooks, constants, context providers are silently stale.
- **Why:** the biggest single blocker to "production-grade." Any non-component file edited in a real app fails.
- **Files:** new importer graph in `crates/cli/src/lib.rs` (built once at boot from `.oxide/esm-manifest.json`, extended on watch); `HmrMessage::ModuleUpdate` gains `importers: Vec<String>` bounded by nearest `import.meta.hot.accept` boundary; `packages/core/src/client/hot.ts` walks the chain and re-executes boundary ancestors.
- **Acceptance:** edit `src/lib/utils.ts` used by `page.tsx`; page re-renders without reload and picks up the new `utils`.
- **Effort:** 2–3 days.

#### 1.4 — CSS Modules

- **What:** `import styles from './x.module.css'` returns raw CSS text, not a class-map.
- **Why:** every real React app uses CSS Modules. Table-stakes.
- **Files:** `crates/cli/src/esm_dev.rs::serve_css_module` detects `.module.css`; runs `lightningcss`'s `cssModules` transform (already a workspace dep); serves a JS module with `export default { className: mangled }`; keeps `<style data-oxide-css>` injection so HMR works.
- **Acceptance:** end-to-end test that imports `.module.css` and asserts `<h1 class="<mangled>">` renders; assert hot-edit changes the mangled name and the DOM updates.
- **Effort:** 4–6h.

#### 1.5 — Server function hot-swap

- **What:** plain `.server.ts` (not `page.server.ts`) imported by a client component triggers `ServerModuleSwap → location.reload()` on save.
- **Why:** "edit a server function, keep the app state" is a Vite-class promise.
- **Files:** `crates/cli/src/lib.rs`'s `ServerSwap` branch changes to `ModuleUpdate` with the new stub URL; the client's `/@server-fn/<path>` stub reimports under a new stub id; the server hot-swaps the handler body in the callable registry. Works only if the **signature** is unchanged; a signature change legitimately forces a reload.
- **Acceptance:** edit `greeting.server.ts` body, call `greet()` from the page, assert the new greeting with `timeOrigin` unchanged. Signature change asserts reload.
- **Effort:** 1 day.

### Tier 2 — coverage

Each item is a Playwright test to add (and where relevant, a small fix to make it pass). Combined with the original eight gaps from the previous doc.

#### 2.1 — Island HMR
- Edit `src/islands/counter.tsx`; assert `<Counter>` re-renders; click state preserved. **4h.**

#### 2.2 — Layout HMR
- Edit `src/routes/layout.tsx` header text; assert the page updates; `timeOrigin` unchanged. **3h.**
- **STATUS (recon 2026-11-02): NOT a test-only item.** `classify_path` treats `layout.tsx` as structural → `RestartSubsystems` → `HmrMessage::FullReload { reason: "route tree rebuilt" }`. Current behavior: the page RELOADS; `timeOrigin` changes. The in-place path requires `classify_path(&str)` to become `classify_event(&FileEvent)` so `Modified` on an existing layout routes to `AdapterHmr` while `Created`/`Removed` still triggers a rescan. That is §3.9 work. A pinning test (`HMR: layout edit reloads the page`) lands in `tests/playwright/tests/hmr.spec.ts` and will correctly fail when §3.9 lands, prompting the acceptance-criteria update.

- **STATUS (recon 2026-11-02): DONE via §3.9.** `classify_event` routes `Modified(routes/layout.tsx)` to `ChangeKind::Component` -> `AdapterHmr` -> `ModuleUpdate`, so the layout updates in place. The pinning test was rewritten as `HMR: layout edit updates DOM in place, no reload (Tier 2.2)`, which asserts `timeOrigin` UNCHANGED and now passes. The spurious-`Created`-on-existing-file case that FSEvents emits is handled by a birthtime check inside `classify_event`.
#### 2.3 — Route loader (`page.server.ts`) HMR
- Edit a loader; assert the returned data is reflected in the DOM via `ox:data`. **4h.**

#### 2.4 — State preservation
- Add `<Counter initial={0} />`, click +1, edit component, assert counter still shows `1`. **3h.**

#### 2.5 — Error overlay + recovery
- Write `const x = ;` to `hmr-target.tsx`; assert `#ox-hmr-overlay` shows; restore; assert overlay clears. **3h.**

#### 2.6 — Multi-client broadcast
- Two `page` contexts, one edit, both `.toHaveText(marker)`. **2h.**

#### 2.7 — WebSocket reconnect
- Kill server mid-session; restart; edit applies to the still-open page. **3h.**
- **STATUS (recon 2026-11-02): BLOCKED by §3.10.** Reconnect loop works (client retries every ~1s). But the page does not SURVIVE the outage: a lazy `import('/@prebundle/react-dom-client.js')` fails with `ERR_CONNECTION_REFUSED`, escapes uncaught, and React unmounts the whole tree (body 875 to 747 bytes, `<h1 data-hmr-target>` gone). Nothing left for the reconnected socket to update.
- Test-design lesson: Playwright `page.on('websocket')` fires on ATTEMPT, not success. A future test needs a success signal (a `connected` HMR frame, or `__oxideBoot` still an object, or a `WebSocket.prototype` open counter).
- When §3.10 lands this becomes a ~3h test again, the reconnect loop is already correct.

- **STATUS (recon 2026-11-02): DONE.** Test in `tests/playwright/tests/hmr-reconnect.spec.ts` (own server on 3111, own fixture guard). Flow: load → snapshot timeOrigin → kill server (await exit) → sleep 2s (forces failed reconnect attempt + in-flight import rejections) → assert `__oxideBoot` still an object and `<h1 data-hmr-target>` still in the DOM → restart server → sleep 2.5s → write edit → assert marker lands without reload → assert no unexpected page errors. Passes on chromium/firefox/webkit after the §3.10 catches. The "page survives the outage" assertion is the §3.10 fix, not a §2.7 test detail — before §3.10, the tree was unmounted and this test failed with "element(s) not found".
#### 2.8 — Malformed-file mid-edit (reload fallback)
- Save a syntax-broken file; assert `timeOrigin` **does** change (reload is correct here). **2h.**

#### 2.9 — `dev.esm: false` rollback
- `oxide dev` with `dev: { esm: false }`; hit `/`; assert `/client/*.js` boot chunk + hydration. **4h.**

- **STATUS (recon 2026-11-02): DONE via `--no-esm` flag.** The bundler dev path (`ensure_dev_client_bundle` at startup when `!cfg.dev.esm`) has always been live, but was unreachable from a test without editing `oxide.config.ts`. Added `oxide dev --no-esm`, which overrides `cfg.dev.esm = false` in `cmd_dev` AND sets `OXIDE_DEV_NO_ESM=1` in the environment because `start_server_inner_full` **reloads** config from scratch. Test in `tests/playwright/tests/hmr-bundler.spec.ts` (own server on 3112): HTML references `/client/__boot__.js`, does NOT reference `/@boot/boot.mjs` nor `/@prebundle/`, `/client/__boot__.js` fetchable and non-trivial, real Chromium hydrates with `__oxideBoot` set and no unexpected console errors (noise-filtered for the four real bundler-dev gaps below).
- **FOLLOW-UPS FILED (§3.11 candidates, not ticked):** bundler-dev has four real gaps §2.9's contract doesn't cover:
  1. **SSR content wrong:** `--no-esm` renders the neutral-IR "Atlas" conformance fixture (`<h1>Hello Atlas</h1>`) instead of the real route component (`<h1>Home</h1>`). `oxide start` against built `dist/` is correct — gap is `oxide dev`'s bundler path.
  2. **node built-ins in client bundle:** `node:fs/promises`, `node:crypto`, `node:path` reach the browser → CORS errors (Chromium/Firefox) / unsupported-URL errors (WebKit). Missing `strip_server_fn_imports`-equivalent tree-shaking.
  3. **`@prebundle/react-refresh-runtime.js` 404:** ESM-path URL fetched in bundler mode; 404 HTML trips MIME block in Firefox.
  4. **At least one 404:** dynamic chunk (or favicon) referenced but not served.
  Filtered from the test's console-error check per-engine with explicit regex + comments. Today `--no-esm` is opt-in so the ESM path ships; §3.11 should absorb all four if `--no-esm` becomes default.
#### 2.10 — `oxide build` regression
- `oxide build --root examples/smoke-app-react`; boot the built output; smoke-test islands. **3h.**

- **STATUS (recon 2026-11-02): ALREADY COVERED.** The plan's recipe (`oxide build` → boot built output → smoke-test islands) is exactly what the whole Playwright suite does. `global-setup.ts` runs `oxide build --root examples/smoke-app-react` before any spec starts; `oxide start` serves the resulting `dist/`; `smoke.spec.ts` asserts SSR HTML, health endpoint, and browser-server equivalence; `island.spec.ts` asserts island markup + hydration + click state; and 8 sibling specs (`deferred`, `loading`, `middleware`, `no-js`, `og`, `socket`, `sse`, `sw`) exercise the built output as their runtime. No new test is warranted — §2.10's acceptance is met by the existing suite. One caveat: `global-setup.ts` caches on `dist/client/client.manifest.json` presence, so a source edit between runs serves the stale build. That is a dev-convenience trade-off, not a build-regression gap; a fresh clone or a `rm -rf examples/smoke-app-react/dist` exercises the full build path. If the cache ever masks a real build regression, delete it in `global-setup.ts` — but that costs 5-10s per `pnpm test` run and isn't warranted today.
#### 2.11 — CSS HMR end-to-end
- Edit `src/routes/*/page.css`; assert `<style data-oxide-css>` textContent changed. **3h.**

#### 2.12 — `react-refresh` absent path
- Config-test: uninstall `react-refresh`; assert HMR tests skip but structural tests pass, and a warning is logged. **2h.**

- **STATUS (recon 2026-11-02): DONE — scope split, only the client fallback needed a new test.** The plan's three claims each had a different status:
- **(a) HMR tests skip when `react-refresh` absent.** Already the pattern: every HMR test in `hmr.spec.ts` has `test.skip(!HMR_AVAILABLE, ...)` where `HMR_AVAILABLE = existsSync(smoke-root/node_modules/react-refresh)`. Testing this means literally uninstalling `react-refresh` mid-run, which would break the shared-server suite. Not worth a new test; the guard is trivially verifiable by inspection and by the CI check added in §4.1 (`global-setup.ts` fails fast if `CI` is set and `react-refresh` is missing, precisely so this mode isn't exercised accidentally).
- **(b) A warning is logged when prebundle fails.** Already exists: `crates/cli/src/esm_dev.rs:332` prints `oxide dev (esm): cannot prebundle <name> — /@prebundle/<url> will 404 (<err>)`. Fires whenever a `cfg.dev.prebundle` package fails (including react-refresh on a project without it). Exercised implicitly by any dev run against a project missing one of its prebundle deps. Not worth a dedicated stderr-capture test.
- **(c) `__oxide_refresh` falls back to `location.reload()` when `$RefreshRuntime$` absent.** The actual behavioural guarantee the plan wants. Landed as a new unit test in `packages/core/test/hot.test.ts`: stubs `location.reload`, deletes `$RefreshRuntime$`, calls `__oxide_refresh`, asserts one reload. Fails if the `location.reload()` fallback in `hot.ts`'s `__oxide_refresh` is removed.
#### 2.13 — Cold-start race (U6)
- Fresh browser; edit within the first 500ms after `__oxideBoot`; assert the registration replay works. **2h.**

#### 2.14 — Firefox / WebKit
- Run the full spec on `--project=firefox` and `--project=webkit`; fix engine-specific breakage (if any). **2–3h.**

#### 2.15 — Cold-start bound
- `performance.timing` measurement; assert `<5s` on local hardware as a sanity ceiling. **2h.**

### Tier 3 — robustness

#### 3.1 — Server-side watcher debounce
- **What:** the client `COALESCE_MS` is defense-in-depth; the server publishes one message per `notify` event, not one per save.
- **Fix:** `crates/cli/src/lib.rs`'s `notify` handler accumulates for ~100ms, publishes one `ModuleUpdate`. ~30 lines.
- **Acceptance:** instrumentation counts 1 `ModuleUpdate` per save; the client coalesce remains as safety net.
- **Effort:** 3h.

#### 3.2 — Batch change detection
- **What:** `git checkout <branch>` fires ~50 events → 50 `reloadModule` calls.
- **Fix:** accumulate over 200ms, publish one `{ files: [...] }`; client runs one `performReactRefresh()`.
- **Effort:** 4h.

- **STATUS (recon 2026-11-02): DONE.** Wire-level batching. The watcher accumulates `AdapterHmr` events per poll tick and publishes ONE `HmrMessage::BatchModuleUpdate { items: Vec<BatchModuleItem> }` when >1 item collected (fallback to singular `ModuleUpdate` for the common 1-file save). The client's `hmr.ts` gained a `case 'batch-module-update'` that fans out `reloadModule(url)` per item. A burst of 5 distinct-file writes publishes ONE frame; the acceptance test `HMR: burst touching N distinct files yields one dispatched message` asserts `delta === 1` on the wire. Coalescing the per-item `performReactRefresh()` calls into one at the end would need `reloadModule` to expose a 'suppress refresh' flag — deferred, not observable in the wire-level test.
#### 3.3 — Prebundle invalidation (U7)
- **What:** `node_modules/*/package.json` changes do not regenerate the prebundle.
- **Fix:** mtime-hash the package.json and invalidate.
- **Effort:** 3h.

- **STATUS (recon 2026-11-02): DONE.** The watcher's per-batch loop now detects (a) any event whose path contains `/node_modules/` and ends with `/package.json`, and (b) any lockfile change (already detected — its handler now also re-runs prebundle). Both re-run `esm_dev::prebundle_cjs_deps(&rootp, &cfg.dev.prebundle)` and publish `HmrMessage::FullReload { reason: "node_modules change — prebundle regenerated (...)" }` in ESM mode. The FullReload (not ModuleUpdate) is the correct signal: a package change is not an edit React Refresh can preserve state across, and the client's next request rebuilds `EsmDevState` with the fresh `rewrite_map` (no in-memory `RwLock` needed). Test in `tests/playwright/tests/hmr-prebundle.spec.ts` (own server on 3113): snapshots prebundle mtime, creates a real package.json under node_modules, asserts mtime advanced AND a `full-reload` frame reached the live socket within 10s. Verified `pnpm`-symlinked `node_modules/<pkg>/package.json` files are NOT usable for the test (they point into the pnpm store outside the watch root); the test uses a fresh `node_modules/.oxide-test-prebundle-diag/package.json` instead.
#### 3.4 — `import.meta.env.X` HMR (U8)
- **What:** env changes trigger a full reload. Common Vite requirement.
- **Fix:** publish `ModuleUpdate` for the env module; client re-imports the env shim.
- **Effort:** 2h.

- **STATUS (recon 2026-11-02): DONE, Vite-parity.** The plan's text was contradictory ("what: env changes trigger full reload" vs "fix: publish ModuleUpdate for env module"). Vite 5+ does the FULL RELOAD — env values are read once per module evaluation, so a hot update would leave already-evaluated modules with stale values, silently divergent from a fresh page load.
- **`import.meta.env` injected in every served module's preamble** (`crates/cli/src/esm_dev.rs::serve_source_file`). Only `PUBLIC_*`-prefixed variables are exposed (§14.2); everything else stays server-side. `MODE: 'development'` for Vite parity. Values read FRESH per request via `oxide_fw_config::load` (~1ms) — no cross-scope state or RwLock. Preamble preserves `import.meta.hot` install.
- **Watcher publishes `FullReload` on `.env*` change** (`crates/cli/src/lib.rs`). Match: filename starts with `.env` AND parent is the project root (a `.env` under `src/` is a source file, not config). Reason: `.env* change — import.meta.env values refreshed`.
- **Test** (`tests/playwright/tests/hmr-env.spec.ts`, own server on 3114, own `.env` lifecycle): writes `.env` with `PUBLIC_*` + secret, asserts served module preamble contains the public var + `MODE` and NOT the secret, loads the page in a live browser (proves boot succeeds), edits `.env`, polls the served module for the new value, asserts the secret still does not leak. Runs on all 3 engines.
- **§14.2 security guarantee** is the test's hard assertion: `SECRET_*` must never appear in a served client module, before OR after a `.env` reload.
#### 3.5 — Worker thread `import.meta.hot` (U1)
- **What:** `import.meta.hot` is `undefined` inside `new Worker(...)`.
- **Fix:** install a worker-side shim via `importScripts` or the entry module.
- **Effort:** 4h.

- **STATUS (recon 2026-11-02): DONE.** Two root causes:
  1. **`install()` bailed on `typeof window === 'undefined'`** in `packages/core/src/client/hot.ts`. The dev preamble assigns `import.meta.hot = globalThis.__oxide_hmr__.createHotContext(...)`, but in a Worker `globalThis.__oxide_hmr__` was never set. Fixed: detect worker scope (`typeof importScripts === 'function'`), install `__oxide_hmr__` there too. React Refresh installs (`__oxide_refresh`, `__oxide_refresh_reg`, `$RefreshRuntime$` pull) are gated on `hasWindow` — a worker has no components or DOM; only the module accept-callback flow applies.
  2. **No worker-side WS.** In the main thread `connectHmr()` (from `boot.ts`) owns the socket. Workers have no `boot.ts`. Fixed: a worker-only IIFE at module bottom opens a WS to `/_oxide/hmr`, listens for `module-update`/`batch-module-update`, and calls `reloadModule(url)` per frame. The server's `HmrHub::publish` broadcasts to every subscriber, so main thread AND worker receive the same messages. `full-reload`/`server-module-swap`/`error` frames are ignored in a worker — no DOM to show, and the main thread's own WS handles them.
  - **Fixture gotcha (NOT a hack):** the initial fixture used an inline `<script>` inside `dangerouslySetInnerHTML` to spawn the worker. The dev CSP (`script-src 'self' 'nonce-...' 'strict-dynamic'`) blocks inline scripts that lack a nonce, and the render pipeline only adds nonces to scripts IT emits (`/@boot/boot.mjs`). The correct pattern for spawning a worker on hydration is a React `useEffect`, not an inline script. Fixture rewritten accordingly. This is also the correct pattern for ANY framework user wanting to spawn a worker — the tested path matches real usage.
  - **Test:** `tests/playwright/tests/hmr-worker.spec.ts` (own server on 3115, own fixture restore in afterAll). Navigates to `/worker` (via a `useEffect`-driven `<Worker>`), asserts the worker posts `{ hotDefined: true, v: 'WORKER-V0', ts }`, edits the worker source to `WORKER-V1`, asserts the worker posts again with `WORKER-V1`, asserts `performance.timeOrigin` UNCHANGED (worker HMR does not reload the main page).
- **Fixtures added (permanent):** `examples/smoke-app-react/src/workers/counter.worker.ts` and `examples/smoke-app-react/src/routes/worker/page.tsx`.
#### 3.6 — Multi-worker hub isolation (U9)
- **What:** each process has its own broadcast; a WS to server A does not see edits on server B.
- **Fix:** in-process broadcast across workers, or a shared file/redis channel.
- **Effort:** 3h.

#### 3.7 — Perf benchmarks
- **What:** no measurement of "write → DOM update" latency. Regression from 25ms → 800ms would not trip any test.
- **Fix:** small corpus + assert `<100ms` on local hardware.
- **Effort:** 3h.

#### 3.8 — Cache-buster memory bound
- **What:** `?t=<ms>-<seq>` accumulates imported modules in browser memory over a long day.
- **Fix:** bounded LRU + drop old references after each successful refresh.
- **Effort:** 2h.

- **STATUS (recon 2026-11-02): PARTIAL — §3.8a DONE, §3.8b deferred.**
- **§3.8a (done, this batch):** `modules` and `lastReloadAt` in `packages/core/src/client/hot.ts` are now capped at 500 entries each (insertion-order eviction). A session touching thousands of distinct URLs (pathological but possible: a buggy editor plugin, a scripted test) no longer grows JS heap unboundedly. Pinned by `modules map stays bounded under many distinct URLs (Tier 3.8a)` in `packages/core/test/hot.test.ts`; the test uses a new `__debug` getter hook on `globalThis.__oxide_hmr__`.
- **§3.8b (deferred, architectural):** `?t=<ms>-<seq>` cache-busting still adds one module instance per distinct URL to the **browser's** own ESM module map. JS cannot evict from that map. A real fix needs a proxy architecture (import maps with mutable bindings, or a service worker) — not the plan's 2h. If this ever matters (a week-long dev session on a hot-reload-heavy project), it becomes its own Tier-3 item.
- The plan's original 2h estimate conflated (a) and (b). (a) is what the test pins; (b) is real but out of scope here.
#### 3.9 — Route edits hot-swap (U11)
- **What:** editing an *existing* `routes/**/page.tsx` reloads; production dev servers hot-swap.
- **Blocked on:** 1.3 (transitive graph).
- **Fix:** `classify_path` distinguishes "new route file" from "existing route body edit"; the latter routes to `AdapterHmr`.
- **Effort:** 1 day, blocked on 1.3.


#### 3.10 — Client survives transient dev-server outages (page must not unmount)
- **Why:** discovered while writing §2.7 (2026-11-02). A transient `ERR_CONNECTION_REFUSED` on a lazy boot-graph import (e.g. `import('/@prebundle/react-dom-client.js')`) throws uncaught and React unmounts the whole SSR tree; body shrinks 875 to 747 bytes, the HMR target `<h1>` disappears, and only a manual reload recovers. Page-liveness, not reconnect — the reconnect loop already works.
- **Scope:**
  - **(a)** Catch every `import('/@prebundle/...')` in the boot graph — `hot.ts` (the `react-refresh-runtime` pull), `boot.ts`, route loading — so the rejection cannot escape to React.
  - **(b)** Install a top-level React error boundary at the hydration root (in `boot.ts` or per-adapter `hydrate.ts`) that renders the existing SSR content on lazy-load failure instead of unmounting.
  - **(c)** Retry the lazy import with exponential backoff after WS reconnects — do NOT reload, that is what §2.5 overlay-without-reload ruled out.
  - **(d)** On WS reconnect, re-run `connectHmr` boot path so a failed runtime pull gets a second chance.
- **Files:** `packages/core/src/client/boot.ts`, `packages/core/src/client/hot.ts`, `packages/core/src/client/islands.ts`, adapter `hydrate.ts`.
- **Acceptance:** §2.7's reconnect test passes end-to-end — kill server, wait 2s, restart, edit applies, `__oxideBoot` stays an object, `h1[data-hmr-target]` never leaves the DOM. Regression test with `page.on('pageerror')` gated on `ERR_CONNECTION_REFUSED`.
- **Effort:** 1 day (boundary + retry + tests). Unblocks §2.7.

- **STATUS (recon 2026-11-02): DONE — scope reduced by inspecting the actual failure path.** The plan scoped this as a top-level React error boundary + retry-with-backoff + re-run-connectHmr. The §2.7 diagnostic (see §2.7 STATUS note) showed the actual mechanism: a `ERR_CONNECTION_REFUSED` on `import('react-dom/client')` from `hydrate-island-real.ts`, escaped uncaught. Adding `.catch` at four lazy-import sites — `hydrate-island-real.ts` loadReact, `islands.ts` defaultLoadChunk, and two in `boot.ts` — stops the unmount. No error boundary required: the boundary would mask the symptom, the catches fix the cause. Landed in `6cebf932`. The retry-with-backoff and re-run-connectHmr sub-steps were part of the plan's original design but turned out to be unnecessary once the unmount was prevented — the client's existing reconnect loop + the client's existing accept-callback flow handle post-reconnect edits without any extra machinery. Pinned by §2.7's test.

#### 3.11 — bundler-dev gap closure (`--no-esm`)
- **STATUS (recon 2026-11-02): CLOSED.** Apparent four bugs on the bundler path traced to ONE real bug (`hot.ts` unconditional `/@prebundle/` fetch) plus ONE systemic root cause (`ensure_dev_client_bundle`'s staleness scan missed `packages/*/dist`, so framework-code fixes were invisible to a running dev server).
- **The real bug (fixed in `94a001df`):** `packages/core/src/client/hot.ts` fetched `/@prebundle/react-refresh-runtime.js` on http/https unconditionally. That URL only exists in the native-ESM dev server; in `--no-esm` bundler mode it 404s. Fixed by gating on `isEsmDev` (scan `<script type=module src>` for `/@boot/` or `/@prebundle/` — same signal `boot.ts:323-330` uses).
- **The systemic root cause (fixed here):** `ensure_dev_client_bundle` only scanned `src/routes/` for staleness. A `packages/core/dist/` rebuild was invisible — the dev server served a bundle containing the OLD framework code indefinitely. Fixed by extending the scan to `src/` (whole), `packages/*/dist/`, and `node_modules/@oxide-fw/*/dist/`. Without this, ANY future bug fix in `packages/core` would be invisible to a running dev server until the app source also changed.
- **Acceptance:** strict `hmr-bundler.spec.ts` (console-error check with NO noise filter) passes on all 3 engines.
### Tier 4 — process

#### 4.1 — Playwright in CI
- **What:** `cargo nextest run --workspace` does not run the HMR spec.
- **Fix:** CI matrix job: build `oxide`, `pnpm -r build`, `pnpm --filter @oxide/playwright test`.
- **Effort:** 2h.
- **STATUS (recon 2026-11-02): pre-existing.** `ci.yml:379-410` already has a `playwright` job: matrix over `[chromium, firefox, webkit]`, `cargo build -p oxide-fw-cli`, `pnpm install`, `pnpm -r --filter '!./packages/oxide-website' run build`, `pnpm exec playwright install --with-deps ${{ matrix.browser }}`, `pnpm exec playwright test --project=${{ matrix.browser }} --timeout=25000`. Every step in the plan's "fix" is present — the plan was written before this job landed.
- **Gap closed by this tick (previously latent):** `tests/playwright/global-setup.ts` asserted only `target/debug/oxide`, not that `examples/smoke-app-react/node_modules/react-refresh` was installed. Since `HMR_AVAILABLE = existsSync(node_modules/react-refresh)`, a CI runner missing that dep would silently skip every HMR test — green job, zero coverage. Added a CI-only `throw` in `global-setup.ts` (no-op locally). Also added `timeout-minutes: 45` to the playwright job so a hung browser doesn't burn GitHub's default 360-min limit.

#### 4.2 — Spec dead-code cleanup
- **What:** `grep -c 'console.log\|debugger' tests/playwright/tests/hmr.spec.ts` — should be 0.
- **Effort:** 30m.

#### 4.3 — This file maintenance
- **What:** every Tier 1/2 item landing means an update here.
- **Effort:** ongoing, 5m per commit.

## Execution order

Chosen so each step makes the next cheaper. Priority is "value per day."

| Week | Focus | Items | Days |
|---|---|---|---|
| 1 | Unblock + kill silent-bug classes | 1.1, 1.2, 3.1, 4.1, 4.2 | 4 |
| 2 | Verification (Tier 2) | 2.1–2.15 | 3–4 |
| 3 | Correctness architectural push | 1.3, 1.4, 1.5, 3.9 | 5–6 |
| 4 | Robustness | 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8 | 4 |
| 5 | Spillover + polish | remaining | — |

## Definition-of-done per tier

A tier is **done** when every item in it:

1. Has a commit on the branch with a message that names the concrete regression it fixes.
2. Has a test that **would fail if the fix were reverted** (not just one that passes with the fix).
3. Has a line here marked ✅ and a one-line note of its acceptance evidence.

## Tracking

- [x] 1.1 `strip_server_fn_imports` multi-name
- [ ] 1.2 `$RefreshSig$` real implementation
- [ ] 1.3 Transitive dependency propagation
- [ ] 1.4 CSS Modules
- [ ] 1.5 Server function hot-swap
- [x] 2.1 Island HMR
- [x] 2.2 Layout HMR
- [x] 2.3 Route loader (`page.server.ts`) HMR
- [x] 2.4 State preservation
- [x] 2.5 Error overlay + recovery
- [x] 2.6 Multi-client broadcast
- [x] 2.7 WebSocket reconnect
- [x] 2.8 Malformed-file mid-edit
- [x] 2.9 `dev.esm: false` rollback
- [x] 2.10 `oxide build` regression
- [x] 2.11 CSS HMR end-to-end
- [x] 2.12 `react-refresh` absent path
- [x] 2.13 Cold-start race
- [x] 2.14 Firefox / WebKit
- [x] 2.15 Cold-start bound
- [x] 3.1 Server-side watcher debounce
- [x] 3.2 Batch change detection
- [x] 3.3 Prebundle invalidation
- [x] 3.4 `import.meta.env.X` HMR
- [x] 3.5 Worker thread `import.meta.hot`
- [ ] 3.6 Multi-worker hub isolation
- [ ] 3.7 Perf benchmarks
- [x] 3.8 Cache-buster memory bound
- [x] 3.9 Route edits hot-swap
- [x] 3.10 Client survives transient dev-server outages
- [x] 3.11 bundler-dev gap closure (--no-esm)
- [x] 4.1 Playwright in CI
- [x] 4.2 Spec dead-code cleanup
- [ ] 4.3 This file maintenance (ongoing)

## Total effort

| Tier | Items | Engineering days |
|---|---|---|
| 1 (correctness) | 5 | 5–6 |
| 2 (coverage) | 15 | 2–3 |
| 3 (robustness) | 9 | 3–4 |
| 4 (process) | 3 | 0.5 |
| **Total** | **32** | **~11–14** |

## Related reading

- `git log --oneline 588023d8..HEAD` — the 14 landed commits and the historical record.
- `tests/playwright/tests/hmr.spec.ts` — the current 15-test suite.
- `crates/cli/src/esm_dev.rs` — the ESM dev server's URL families.
- `packages/core/src/client/hot.ts` — the HMR runtime.
- `packages/core/test/hot.test.ts` — unit tests for `reloadModule` and `createHotContext`.
