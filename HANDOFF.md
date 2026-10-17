# Oxide session handoff — SPEC bucket-2 started; router boot **uncommitted and blocked**

**Date written:** 2026-10-17
**Repo:** `oxide` (meta-framework, spec v0.1)
**HEAD at handoff:** `4633124` (`docs(worklog): SPEC-17 — JS plugin bridge (SPEC-17)`)
**Working tree:** **NOT clean** — the client-router boot slice is on disk, compiles, but is
**not committed** because it would break navigation (see §1). Two files modified:
`packages/oxide/src/client/router.ts`, `crates/cli/src/lib.rs`.

Also untracked and not mine: `docs/v0.2-ideas.md`. Leave it.

---

## 0. Read this first

This session was driven by a **spec-compliance scorecard** that the user supplied
(`Oxide v0.1 Spec Compliance Scorecard`, dated 2026-10-16 @ `4633124`). Its verdict:

> ~70% overall, but deep where users don't look (transport, caching, security,
> observability: 75–90%) and shallow where users live (route modules, data layer,
> rendering: 50–60%). None of the seven user-visible live-path gaps were touched
> by the last 138 commits — everything landed *around* them.

The user's instruction was **"no defer, implement everything"**, but the scorecard is
correct and the remaining work is in three buckets:

- **Bucket 1 — finish-what-you-shipped** (wire built-but-unconnected code).
- **Bucket 2 — the user-visible seven** (§8/§9/§10: router boot, nested layouts,
  loading/error/not-found, `route.ts`, middleware, defer values, non-React hydration).
- **Bucket 3 — the numbers** (§21 benchmarks).

**This session started Bucket 2, item 1 (boot `createRouter`) and hit a real blocker.**
Do not "finish" it by attaching the router without reading §1.

---

## 1. The in-flight slice: client router boot — BLOCKED, do not commit as-is

### What is on disk (uncommitted)

`crates/cli/src/lib.rs` — `generate_island_boot_entry` now appends:

```js
import { createRouter } from 'oxide/client';
createRouter({ adapter: clientAdapter }).attach();
```

`packages/oxide/src/client/router.ts` — `RouterOptions.manifest` made **optional**
(it was required but **never read** by `createRouter`; `grep -n manifest router.ts`
returns only the type declaration at line 63).

### Why it is NOT committed — the blocker

`createRouter(...).attach()` installs a `click` listener that intercepts **every**
same-origin `<a href>`, calls `adapter.update(input)`, and pushes history state.
`adapter.update()` delegates to module-level `active?.update(input)` in
`packages/react/src/hydrate.ts:221`. **`active` is only set by `hydrate()` or
`mount()` (`hydrate.ts:203,213`) — and `packages/oxide/src/client/boot.ts` calls
neither.** So `update()` is a no-op: a click would change the URL and the DOM
would not re-render. That is **worse than no router** (a normal page load works).

### The fix to implement (bounded, ~1 slice)

1. In `boot.ts`'s `__oxideBootRun`, **hydrate the root first**, then attach the router.
   - Root element: `document.getElementById('root')` (every adapter's `rootAttrs`
     emits `id="root"` — `packages/react/src/real-adapter.ts:79`, `render.ts:487`).
   - Input: parse `window.__oxide` (`{ route, data }`), which
     `hydration_bootstrap` emits (`crates/render/src/lib.rs:416`). Note `data` is the
     ODF payload's **base64 text**, not decoded data — decide whether to decode it
     (the client has `decodeOdf`) or pass `data: {}` and let the first `goto()`
     fetch `/__data`.
   - Call `state.adapter.hydrate(root, input)`. **Verify** this does not overwrite
     the SSR markup in a way that destroys `<ox-island>` hosts before
     `hydrateIslands` runs — check `createClientRoot` in `packages/react/src/hydrate.ts`
     (~line 130-200) and test it in a browser, not the fake DOM.
2. **Only then** construct the router:
   `createRouter({ adapter: clientAdapter }).attach()`.
3. Add a Playwright/real-DOM test: click a same-origin link → URL changes **and**
   the page content changes. A URL-only change is the bug above.

**Ordering risk:** if `hydrate(root)` replaces `#root.innerHTML`, the islands that
`hydrateIslands` was about to hydrate are gone. The safe order may be
`hydrateIslands()` → `adapter.hydrate(root)` → `createRouter().attach()`, or the
reverse; **determine empirically**, do not guess.

### Other gaps this slice does NOT close

- `RouterOptions.manifest` is still unused. SPA nav fetches `/__data` for data, and
  does **not** preload route chunks from the manifest. Documented gap, not stubbed.
- `goto()` on a route whose loader returns non-200 (`error()`/`redirect()`) is
  untested from the client.

---

## 2. What landed this session (committed, 24 SPEC tasks)

HEAD `4633124`; SPEC-02/04/05/09/10/11/12/13(partial)/14/16/17/20/21/22/24/25 all
landed (see `WORKLOG.md` for per-task entries). Highlights:

- **SPEC-14 HMR end-to-end** — `/_oxide/hmr` WebSocket (hand-rolled RFC6455 upgrade
  + SHA-1), `classify_path`, dev-loop publish, `packages/oxide/src/client/hmr.ts`.
  Verified live: 101 handshake → `connected` → edit CSS → `css-update` frame.
  **Caveat:** `hmr.ts` is imported by nothing and the dev HTML does not inject it —
  the *client* half is written but not wired.
- **SPEC-17 JS plugin bridge** — `plugin:load`/`plugin:call`, `build_js_plugin_hook`,
  composed with the WASM hook. Verified: a `transform` plugin rewrites a marker in
  client+server chunks. **Caveat:** the prerender path bypasses plugins.
- **SPEC-05 forbidden-import guard** — literal-only scan; leak caught, fixtures pass.
- **SPEC-10 i18n** — 307 prefix redirect, `Content-Language`, loader `locale`.
- **SPEC-09 typegen** — `RouteData`/`PageProps` + build hook + per-route `LoaderData`
  (fixed a routes-root resolution bug).
- **SPEC-02** loud `node:` shims + `JSH-V8KIND` build refusal.
- **SPEC-04/12** HTTP remote cache + `HttpCacheHandler` (**drivers built, zero
  callers** — no `cache.handler` config key yet).
- **SPEC-13 partial** mimalloc global allocator (embed/reload not done).
- **SPEC-21** PWA `sw.js` + `pwa-register.js` (**not auto-registered**).
- **SPEC-20** verified ICU4X formatters already present (no change).

### Two bugs found and fixed mid-session (both mine)

1. **Plugin-bridge deadlock** — `Runtime::block_on` called from inside the
   bundler's own tokio runtime hung the build. Fixed with `std::thread::scope`
   (same pattern as `emit_snapshot_blob`, `crates/cli/src/lib.rs:1503`).
2. **Plugin payload path** — worker read `input.path`; CLI sends `input.payload.path`.
   Every plugin saw `""`. Fixed.

---

## 3. Test state at handoff

| suite | last result |
| --- | --- |
| `cargo test --workspace --no-fail-fast` | **1047 passed, 0 failed** (after SPEC-17) |
| `./tests/fixtures/check.sh` | **PASS** (4/4) |
| `./tests/fixtures/check-v8.sh` | **PASS** (10/10, generateParams ×2, action) |
| `packages/oxide` | 83 + HMR client 5 |
| `oxide-core-wasm` | 14 (incl. real blake3 `"abc"` vector) |

**Not re-run since the uncommitted router changes.** `cargo check -p oxide-cli`
is green; the full suite and both gates must be re-run before committing anything.

Known flake: concurrent test binaries collide on the redb CAS
(`Database already open`) and on ephemeral ports. A clean re-run is authoritative;
per-crate isolation confirms real-vs-transient.

---

## 4. Conventions in force (do not violate)

1. **One commit per file.** Conventional style, task id in the body.
2. **Verify the emitted artifact, not the log.** `cargo check` green is never enough.
3. **`git checkout -- examples/` before `git add`**, and
   `find examples -name '.*.oxide.ts' -delete` (tests drop temp files).
4. **No `timeout` on macOS.** Use a background PID + a `sleep N; kill` watchdog.
5. **Do not `pkill -f "target/debug/oxide-cli"`** inside a script while a build may be
   running — it kills `oxide-cli build` itself. Kill by PID.
6. **`.git/index.lock` collisions are real here.** There were **6 concurrent
   `just wr1` + `watchexec` pairs** alive at one point (from Wed/Thu/Fri/today);
   every save of `wr1.sh` fired all of them. `pkill -f "watchexec -w ./wr1.sh"` and
   `pkill -f "just wr1"` clears them. If duplicates reappear, a hidden launcher is
   starting new ones — check `ps aux | grep watchexec` before blaming the code.
7. Heredocs with backticks need `<<'EOF'`; patch multi-line content via temp files +
   Python `str.replace`, never inline Python string literals.

---

## 5. Where to start (ranked)

1. **Finish the router boot (§1).** ~1 slice. It is the highest-value Bucket-2 item
   and the client router + `/__data` are already waiting for it. **Do not attach the
   router without `adapter.hydrate` first** — that is the blocker.
2. **Bucket 2 the rest**, in this order (each is real user-visible):
   - render nested **layouts** (currently not rendered at all);
   - `loading.tsx` / `error.tsx` / `not-found.tsx` wiring (unmatched URLs currently
     return **HTTP 200**);
   - dispatch **`route.ts`** (never dispatched);
   - execute **`middleware.ts`**;
   - fix the **defer registry** (`defer-glue` receives `detail:{id}` with no value);
   - real **hydration for Vue/Svelte/Solid** (only React is real);
   - real **streaming SSR** per framework (all four still `renderToString`).
3. **Bucket 1 quick wins** (built, unconnected):
   - inject `hmr.ts` into dev HTML + send real code in `css-update`/`module-update`;
   - `cache.handler = { type: 'http', url }` config key → `TieredCache::with_remote`;
   - `'use cache'` payload → ODF (currently JSON);
   - PWA auto-registration script tag;
   - plugin transforms on the prerender path.
4. **Bucket 3 (§21)**: snapshot-first boot for ≤50 ms cold start; re-run the HMR
   bench (the `0/8` figure predates SPEC-14 and is stale); **refresh
   `benchmarks/report.md` — it still feeds the CI gate.**

---

## 6. Scorecard reference (do not re-derive)

The user's scorecard is at the top of this document's history; the per-section table
is authoritative. The **seven user-visible live-path gaps** are the ones to fix:
router boot, nested layouts, loading/error/not-found, endpoint dispatch (`route.ts`),
middleware, deferred values, non-React hydration.

`HANDOFF.md` (this file) supersedes the 2026-10-15 handoff, which was 316 commits
stale. If you find a claim here contradicted by the tree, **trust the tree** and
note the drift in `WORKLOG.md`.
