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

## CSS gap (Phase 5 work, not a Phase 4 regression)

**The dev server still runs `ensure_dev_client_bundle` at cold start** so
the CSS chunk exists at `/client/index-<hash>.css` for `<link>` tags. This
means:

- Cold start is still ~2s (the Rolldown bundle pass), not the ~1.2s the
  plan's table projected — that figure assumed a pure-ESM cold path.
- Saves are still ~25ms when React Refresh succeeds (Phase 3 gain), and
  ~500ms (full reload) when the component doesn't register cleanly.

Closing the CSS gap (serving per-route CSS directly from `/src/**/*.css`
without an initial bundle) is Phase 5. It is intentionally out of scope
here so the ESM path can ship without regressing any current app.

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
