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

## Phase 5b (deferred)

A project that `import './styles.css'` inside a route component requests
`/src/styles.css` and gets a 404. This is not a Phase 5 regression — the
ESM path served the same 404 in Phase 1. The fix is a `/@css/**` URL
family piped through LightningCSS; tracked for a follow-up if a real user
hits it.

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
