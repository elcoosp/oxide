# Oxide session handoff — SPEC bucket-2 COMPLETE

**Date:** 2026-10-17
**Status:** All seven user-visible Bucket-2 gaps and every previously-deferred
partial item are implemented and tested. No known stubs remain on the
user-visible live path.

Supersedes `docs/HANDOFF-bucket2-session.md` (which recorded 6 of 7 done).
The tree is green: 260 Rust lib tests + 240 TS package tests + end-to-end
smoke-app proofs.

---

## What is now fully implemented

### 1. Router boot (§8.4) — COMPLETE
`bootWithRouter` hydrates the SSR root via `adapter.hydrate` **before**
attaching `createRouter`, so SPA navigation swaps the DOM. 5 tests in
`packages/oxide/test/boot.test.ts`.

### 2. Nested layouts (§8.2, §16.3) — COMPLETE
`RenderInput.layouts: string[]` flows router → `PageRenderInput` →
`FrameworkRenderInput` → adapter. `makeRealAdapter` nests
`L1(L2(L3(page)))` (reverse-wrap). React expands `{Component, props}`
descriptors into elements. Legacy `layoutFor` still works. Populated on
**both** live and prerender paths. 5 tests in `nested-layouts.test.ts`.

### 3. not-found.tsx (§10.2 step 5) — COMPLETE
Unmatched URLs return **404** (was silently 200 — the framework-render path
hardcoded `stream(200, body)`). The project's `not-found.tsx` renders with
its own layout chain. Smoke-app proof: `/this-does-not-exist` → 404 + page.

### 4. error.tsx (§10.2 step 5) — COMPLETE
Nearest-ancestor `error.tsx` resolves to `error_route_id` → `input.errorRoute`;
the adapter's catch renders it with `{ message, route, status }`. Wired on
live + prerender. 4 tests in `error-boundary.test.ts` (incl. the `props`
scope-regression guard).

### 5. route.ts endpoint dispatch (§8.1, §10.2 step 7) — COMPLETE
Worker `endpoint:run` → `EndpointHost` → `EndpointDispatcher` trait → CLI
impl, dispatched before the render phase. Named per-method exports or default.
Smoke app: `/api/users` GET→JSON, POST→echo 201.

### 6. middleware.ts (§8.1, §8.5) — COMPLETE
Worker `middleware:run` → `MiddlewareHost` → `MiddlewareDispatcher`. Runs
**before** actions and loaders (ordering: middleware → action → loader →
endpoint → render). Supports `undefined`/`{redirect}`/`{rewrite}`/
`{headers}`/`{locals}`/`Response`.
- **locals** reach loaders, actions, and route.ts handlers
  (`LoaderRunInput.locals` → worker `event.locals`; `ActionHost`/`EndpointHost`
  forward too).
- **`{ rewrite }`** re-dispatches through the normal match path with an
  `x-oxide-rewrite-hop` guard (cap 8) so a self-rewrite cannot loop.
Smoke app: `/greeting` prints "hello, ada" (locals); `?gated=1` → 307 /login.

### 7. Defer values (§7.1, §9.2) — COMPLETE
Previously: `ox:swap` carried `{id}` only (resolved `undefined`) and the
client never wired loader data into the glue at all.
- Worker **awaits** every `defer()` promise and returns `{ id: value }`.
- `LoaderOutput.deferred` → CLI injects
  `<script type="application/oxide-deferred">{json}</script>` (serde + `<`
  escaped) — even when there is no boot module.
- `attachDeferGlue` parses the script on attach **and** on every `wire()` (so
  slots created after the script is parsed still resolve); a rejected defer
  is `{ __oxide_deferred_error }` and rejects the slot.
- `hydrate`/`update` now call `glue.wire(data)` in all four adapters — the
  production bug that meant no slots ever existed.
4 tests in `defer-script.test.ts` + 6 in `defer-glue.test.ts`.

### 8. Non-React hydration (§10.3) — COMPLETE
- Vue: `createSSRApp().mount()` (`vueHydrateIsland`)
- Svelte: `mount(..., { hydrate: true })` (`svelteHydrateIsland`)
- Solid: `hydrate(fn, el)` (`solidHydrateIsland`)
Each wired into `real-adapter.ts` and overridden on `clientAdapter` (the boot
entry imports `clientAdapter`).

### 9. Loading boundaries (§8.4) — COMPLETE
- `RouterOptions.onNavigateStart/onNavigateEnd` hooks fire around the
  `/__data` fetch and the tree swap.
- `FrameworkAdapter.renderLoading` paints a route's `loading.tsx` into an
  overlay; `bootWithRouter` finds the longest URL-prefix match (trailing-slash
  normalized) and tears it down on nav end.
- CLI bundles `loading.tsx`, emits a URL-prefix → component map into the boot
  entry.
- `renderLoading` implemented for all four frameworks (React `createRoot`,
  Vue `createApp`, Svelte `mount`, Solid `render`).
3 tests in `loading-boundary.test.ts`; smoke app: `routes/slow/loading.tsx`.

### 10. Streaming SSR (§7.1) — COMPLETE
- `RealAdapterConfig.renderStream` — chunked body renderer, `renderBody`
  fallback.
- React 19: `renderToReadableStream` (`renderReactToStream`).
- Vue: `@vue/server-renderer` `renderToStream` (`renderVueToStream`, Node +
  web stream normalization).
- Solid 2: `@solidjs/web` `renderToStream` (`renderSolidToStream`).
- Svelte 5: no streaming API — falls back cleanly to `renderToString`
  (documented; the interface is uniform).
The worker already forwarded chunks as raw binary frames; the adapter's
`render()` now pumps `renderStream` chunks through. Proven end-to-end: the
response is complete HTML with intact layout/island markup.

### 11. Manifest chunk preloading (§6.4) — COMPLETE
`RouterOptions.manifest` is now used: the router matches the URL to a
manifest route pattern (static/param segments, root `index` handling) and
preloads the winning entry's chunks on `prefetch` **and** `goto`. The CLI
boot entry fetches `/client/client.manifest.json` and threads it into
`bootWithRouter`.

---

## Test counts (final)

| suite | result |
| --- | --- |
| oxide-server (lib) | 43 pass |
| oxide-jshost (lib) | 50 pass, 1 ignored |
| oxide-cli (lib) | 100 pass, 7 ignored |
| oxide-render (lib) | 12 pass |
| oxide-router (lib) | 55 pass |
| packages/oxide | 116 pass |
| packages/react | 36 pass |
| packages/vue | 30 pass |
| packages/svelte | 29 pass |
| packages/solid | 29 pass |

End-to-end smoke-app fixtures (all in `examples/smoke-app-react/`):
- `routes/layout.tsx` — nested layout on `/` and `/blog/*`
- `routes/not-found.tsx` — 404 on unmatched URLs
- `routes/error.tsx` + `routes/boom/page.tsx` — OX-500 boundary with message
- `routes/api/users/route.ts` — endpoint dispatch (GET + POST)
- `routes/middleware.ts` + `routes/greeting/` — locals + redirect
- `routes/slow/loading.tsx` + `routes/slow/page.server.ts` — loading boundary

## What remains (honest list)

These are **post-MLP** items the spec explicitly defers, not incomplete work:

1. **Parallel (`@slot`) and intercepting (`(.)`) routes** (§8.5) — spec says
   post-MLP; the tree format reserves slot identity.
2. **React Server Components** (D17) — spec says post-MLP React-only; extension
   points reserved.
3. **V8 backend parity for the new ops** — `endpoint:run`, `middleware:run`,
   `deferred`-awaiting, and `renderStream` are implemented in the Node worker.
   The V8 backend has a hand-ported subset (loader/action/server-fn); adding
   the new ops there is mechanical but not done. The Node backend is the
   default.
4. **Edge/WASM adapter build** (D18) — the core subset compiles to WASM; the
   new ops are Node-worker-only today.

## Conventions (carried forward)

1. **Do NOT `git checkout -- examples/`** when committing smoke-app fixtures —
   it reverts them.
2. A block appended with a marker comment can be **double-appended** — always
   dedupe (`content.find(marker, first + len(marker))`) before committing.
   Hit this 3× this session (endpoint/middleware dispatcher, solid
   renderToStream).
3. Patching a trailing doc-comment by `rfind` can delete the function above it
   — verify the export list after such a patch.
4. Kill background servers by PID, never `pkill`.
5. Node's type-stripping does not type-check — a `const` in `try` referenced
   from `catch` is a runtime `ReferenceError`. The `props` hoist in
   `real-ssr.ts` and its test exist for this.

If a claim here is contradicted by the tree, **trust the tree** and note the
drift in `WORKLOG.md`.
