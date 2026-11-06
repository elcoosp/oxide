# Handoff — multi-framework HMR: Vue, Solid, Svelte e2e green

## Where things stand

**Branch:** `website`
**HEAD:** this commit — the React-only HMR plan (33/33, previous handoff) plus
framework-agnostic HMR: every supported framework (React, Vue, Solid, Svelte)
is now e2e-tested through the ESM dev server and green.
**Working tree:** clean.
**Binary:** `cargo build -p oxide-fw-cli` (dev profile, `debug=0` in this
sandbox — see "Sandbox notes").

## What landed this session (on top of `a1aa83c9`)

### The feature: HMR for every framework

The React HMR runtime was already production-grade (previous handoff). Vue,
Solid and Svelte pages could SSR but had NO client story: the ESM boot entry
hard-coded `@oxide-fw/react`, the manifest never carried `.vue`/`.svelte`
component URLs, and SFCs were never served to the browser at all. This
session made the whole pipeline framework-agnostic:

1. **ESM dev server** (`crates/cli/src/esm_dev.rs`): `/src/*.vue`,
   `/src/*.svelte` — and `/src/*.tsx` under Solid — are pool-compiled
   per request (`serve_pool_compiled_module`, via the Node tooling pool) with
   the same hot preamble + generic self-accept boundary React pages get.
   The boot entry wires the CONFIGURED framework's `clientAdapter` (with a
   working fallback), and the manifest lists `.vue`/`.svelte` page URLs.
2. **Re-render contract** (`packages/core` client): after a `module-update`
   the HMR runtime records the fresh module (`getFreshModule`) and calls the
   boot's `__oxide_on_module_updated` hook; `bootWithRouter` replays the last
   input through `adapter.update`, which re-resolves the route component.
   React is excluded (capability `refresh` — React Refresh patches in place).
   Vue/Solid/Svelte adapters swap the root when the component reference
   changes (`packages/*/src/hydrate.ts`).
3. **Watcher fan-out** (`crates/cli/src/lib.rs`): component edits on the
   component-capability frameworks fan out to importer pages (React keeps
   the skip — React Refresh owns its updates). Loader edits publish the URL
   path pattern with the root special-case fixed (`routes/index` → `/`).
4. **Solid on linux-x64**: `solid-jsx-oxc` ships no linux binary; the worker
   falls back to `babel-preset-solid` (pure JS, per-project) for BOTH SSR
   (tsx-loader) and the pool op — including when the binding is present but
   fails to load. `jsxRuntime: 'preserve'` sucrase pre-strip keeps JSX for
   the preset; an AST pass renames `$$<event>` → `_$<event>` member keys so
   Solid 2's delegated-event runtime sees handlers compiled by the 1.x-era
   preset. §3.6's isolation test — unrunnable before — is now green.
5. **Svelte 5 runtime sharing** (`crates/bundler/src/prebundle.rs`): the
   svelte family prebundles as ONE multi-entry rolldown bundle (shared
   chunks), so `svelte` and `svelte/internal/client` are a single runtime
   instance. Separate per-package bundles inlined two copies and hydration
   died (`init` on a null context). `svelte/internal/flags/legacy` joined
   the prebundle list.
6. **Svelte hydration**: Svelte 5 exposes `hydrate()` as its own entry
   point — `mount(..., {hydrate:true})` silently appends a second tree
   (`packages/svelte/src/real-client.ts`, `hydrate-island-real.ts`).
7. **Browser-safe adapter dists**: `packages/{vue,solid}/src/image.ts` used
   `createRequire('node:module')` — a static node builtin import killed the
   entire client module graph. Replaced with top-level-await dynamic imports
   (sync availability preserved; optional-peer fallbacks kept).
8. **Dev temp modules** (`crates/cli/src/framework_shim.rs`): pool-compiled
   route modules keep source-relative imports; they are re-anchored to the
   real source files as `file://` URLs before writing `.oxide/components/`
   (Node resolved them against the temp dir before).

### Pre-existing bugs the new e2e suite surfaced (all fixed)

9. **Dev incremental bundle leaked `.server` modules**
   (`crates/bundler/src/rolldown_backend.rs`): `IncrementalWorker::start`
   passed `options.out_dir` as the plugin root, so `should_transform`
   rejected every project module and the server-fn stub passes never ran —
   `import "node:fs/promises"` reached the browser in `--no-esm` dev mode.
   Now passes the project root (same as the cold path).
10. **Server-fn id contract** (`crates/compiler/src/oxc_backend.rs`): the
    client stub had no `.id` and the SSR build kept the raw single-arg
    `serverFn(fn)` — the runtime assigned `anonymous-N` on the server while
    the client rendered `undefined`: a guaranteed hydration mismatch on any
    page rendering `fn.id` (`data-greeting-fn`). Client stubs now carry
    `Object.assign(fn, { id })`; SSR/Edge rewrite to the ID-FIRST form
    `serverFn("<static id>", fn)` so both sides agree. smoke + island
    browser tests went red→green with this.
11. **Boot-entry chunk loader** (`crates/cli/src/esm_dev.rs`): the generated
    loader now consults `__oxide_hmr__.getFreshModule(chunk)` before the
    (un-busted) `import(chunk)` — otherwise the browser ESM cache hands back
    the stale page module and HMR re-renders render the old tree.
12. **Boot entry framework adapter** — covered in (1); the spec-level
    regression tests live in `tests/playwright/tests/hmr-{vue,solid,svelte}.spec.ts`
    (7 tests each) with the shared harness `hmr-boot.ts`.

### Also

- Spec fixes: manifest route key (`routes/index/page`), prebundle URL
  sanitizer spelling (`solidjs-web.js`), quote-agnostic svelte import match,
  h1 locators scoped by role+name (the HMR fixture adds a second h1).
- `examples/smoke-app-{vue,solid,svelte}` grew `src/components/hmr-target.*`
  fixtures; their pages import them.
- HANDOFF's §3.6 note is obsolete: the babel fallback makes
  `hmr-isolation.spec.ts` runnable and green on linux-x64.

## Verified test state (this sandbox, chromium, `--workers=1 --retries=0`)

- **Multi-framework HMR**: `hmr-vue` 7/7, `hmr-solid` 7/7,
  `hmr-svelte` 7/7.
- **React HMR group**: `hmr` + `hmr-transitive` 20/20, `hmr-refresh-sig` 1/1,
  `hmr-cold-start` 2/2, `hmr-reconnect` 1/1, `hmr-prebundle` 1/1,
  `hmr-bundler` 1/1, `hmr-isolation` 1/1 (newly runnable).
- **Group B**: smoke 4/4, island 3/3, hmr-server-fn 1/1, hmr-cssmodules 1/1,
  hmr-env 1/1, hmr-worker 1/1, hmr-perf 1/1.
- **Group C**: deferred 1/1, loading 1/1, middleware 3/3, no-js 2/2, og 3/3,
  socket 4/4, sse 2/2, dev-rescan 1/1.
- **Unit**: compiler 48+56, dev 46+1, cli lib 250 (incl. 12 ignored),
  jshost 78+26; `pnpm test:packages` 488/488.
- **Not runnable in this environment** (pre-existing, unchanged):
  - **webkit**: missing GTK4/GStreamer; runs on a normal dev machine.
  - **sw.spec (×2)**: service-worker precache empty in headless chromium
    here; fails identically on the pre-session build.

## Sandbox notes for the next session

1. All previous HANDOFF gotchas still apply (60s beforeAll budgets, serial
   gate, pristine-fixture syncing, `pkill` by PID, core dist rebuild after
   `hot.ts`/`boot.ts` edits, staleness-scan scope).
2. **Disk**: the debug target dir is ~7 GB and this sandbox has ~10 GB. If
   linking fails with "No space left", dedupe stale artifacts:
   `cd target/debug/deps && for stem in $(ls *.rlib | sed 's/-[0-9a-f]*\.rlib//' | sort | uniq -c | awk '$1>1 {print $2}'); do ls -t $stem-*.rlib | tail -n +2 | while read f; do rm -f "${f%.rlib}" "${f%.rlib}.rmeta" "${f%.rlib}.d"; done; done`
   Also build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0` and keep
   `nasm` on PATH (`/home/z/local/bin`) for wasmtime.
3. `hmr-cold-start` flaked once on the FIRST boot after long idle (watcher
   baseline race on a cold `.oxide`); every rerun passed. If it fails, rerun
   before investigating.
4. The first `pnpm install` after a sandbox recycle may fail with
   `ERR_PNPM_PACKAGE_MANAGER_CREATE_SLOT_DIR` (stale overlay dentries);
   `rm -rf node_modules && mkdir node_modules && pnpm install` clears it.

## Where to pick up, in one sentence

The multi-framework HMR rollout is complete and green; next work is
publishing `solid-jsx-oxc` linux binaries (the babel fallback stays as the
portable default), the deferred §3.8b browser ESM-cache proxy, and keeping
the suite green in CI (webkit needs a normal machine).
