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

#### 2.8 — Malformed-file mid-edit (reload fallback)
- Save a syntax-broken file; assert `timeOrigin` **does** change (reload is correct here). **2h.**

#### 2.9 — `dev.esm: false` rollback
- `oxide dev` with `dev: { esm: false }`; hit `/`; assert `/client/*.js` boot chunk + hydration. **4h.**

#### 2.10 — `oxide build` regression
- `oxide build --root examples/smoke-app-react`; boot the built output; smoke-test islands. **3h.**

#### 2.11 — CSS HMR end-to-end
- Edit `src/routes/*/page.css`; assert `<style data-oxide-css>` textContent changed. **3h.**

#### 2.12 — `react-refresh` absent path
- Config-test: uninstall `react-refresh`; assert HMR tests skip but structural tests pass, and a warning is logged. **2h.**

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

#### 3.3 — Prebundle invalidation (U7)
- **What:** `node_modules/*/package.json` changes do not regenerate the prebundle.
- **Fix:** mtime-hash the package.json and invalidate.
- **Effort:** 3h.

#### 3.4 — `import.meta.env.X` HMR (U8)
- **What:** env changes trigger a full reload. Common Vite requirement.
- **Fix:** publish `ModuleUpdate` for the env module; client re-imports the env shim.
- **Effort:** 2h.

#### 3.5 — Worker thread `import.meta.hot` (U1)
- **What:** `import.meta.hot` is `undefined` inside `new Worker(...)`.
- **Fix:** install a worker-side shim via `importScripts` or the entry module.
- **Effort:** 4h.

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

#### 3.9 — Route edits hot-swap (U11)
- **What:** editing an *existing* `routes/**/page.tsx` reloads; production dev servers hot-swap.
- **Blocked on:** 1.3 (transitive graph).
- **Fix:** `classify_path` distinguishes "new route file" from "existing route body edit"; the latter routes to `AdapterHmr`.
- **Effort:** 1 day, blocked on 1.3.

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
- [ ] 2.2 Layout HMR
- [x] 2.3 Route loader (`page.server.ts`) HMR
- [x] 2.4 State preservation
- [x] 2.5 Error overlay + recovery
- [x] 2.6 Multi-client broadcast
- [ ] 2.7 WebSocket reconnect
- [x] 2.8 Malformed-file mid-edit
- [ ] 2.9 `dev.esm: false` rollback
- [ ] 2.10 `oxide build` regression
- [ ] 2.11 CSS HMR end-to-end
- [ ] 2.12 `react-refresh` absent path
- [ ] 2.13 Cold-start race
- [ ] 2.14 Firefox / WebKit
- [ ] 2.15 Cold-start bound
- [ ] 3.1 Server-side watcher debounce
- [ ] 3.2 Batch change detection
- [ ] 3.3 Prebundle invalidation
- [ ] 3.4 `import.meta.env.X` HMR
- [ ] 3.5 Worker thread `import.meta.hot`
- [ ] 3.6 Multi-worker hub isolation
- [ ] 3.7 Perf benchmarks
- [ ] 3.8 Cache-buster memory bound
- [ ] 3.9 Route edits hot-swap
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
