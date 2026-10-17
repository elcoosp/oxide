# Oxide session handoff — SPEC bucket-2 progress

**Date:** 2026-10-17
**Status:** 6 of the 7 user-visible Bucket-2 items are implemented **and**
end-to-end proven on the `examples/smoke-app-react` fixture. One (loading
boundaries) remains a documented gap requiring client route-component
manifest infrastructure or streaming SSR.

This supersedes the router-boot-blocked handoff. The tree is green: every
crate's lib tests, the four adapter suites, and every smoke-app fixture pass.

---

## What landed (verified in tests + smoke-app)

### 1. Router boot (§8.4) — DONE
- `packages/oxide/src/client/boot.ts::bootWithRouter` hydrates the SSR root
  via `adapter.hydrate(root, input)` **before** attaching `createRouter`.
  This sets the adapter module's `active` handle; without it, SPA nav
  changed the URL and the DOM did not re-render (worse than no router).
- The generated island boot entry calls `bootWithRouter(clientAdapter)`.
- New: `packages/oxide/test/boot.test.ts` (5 tests) proves hydrate-before-router,
  hydrate-throw skips router, no-`#root`/no-`window.__oxide` skips, and
  that a same-origin link click dispatches `adapter.update`.

### 2. Nested layouts (§8.2, §16.3) — DONE
- `RenderInput.layouts: string[]` (outermost → innermost) on the wire:
  `PageRenderInput.layouts` → `FrameworkRenderInput.layouts` → `input.layouts`.
- `makeRealAdapter` nests `L1(L2(...(page)))` by wrapping in reverse; legacy
  single-layer `layoutFor(route)` still works.
- `packages/react/src/real-ssr.ts::renderReactToString` expands
  `{Component, props}` child descriptors into React elements (a `const` inside
  `try` referenced from `catch` was a ReferenceError — hoisted).
- The CLI populates `layouts` in **both** dispatch paths: live (`MatchInfo.layout_chain`)
  and prerender (computed from the entry list at `write_prerender`).
- Smoke app: `/` and `/blog/*` render nested in `routes/layout.tsx`.
- Tests: `packages/oxide/test/nested-layouts.test.ts` (5 tests).

### 3. not-found.tsx (§10.2 step 5) — DONE
- The framework-render path used to emit **HTTP 200** for unmatched URLs
  (hardcoded `OxideResponse::stream(200, body)`). Fixed to use the computed
  `status`.
- On no match, the CLI substitutes the project's `not-found.tsx` route id
  (with its own layout chain) so the user's 404 page renders, still with 404.
- Smoke app: `/this-does-not-exist` → HTTP 404 + `data-not-found` page nested
  in the root layout.

### 4. error.tsx (§10.2 step 5) — DONE
- `PageRenderInput.error_route_id` → `FrameworkRenderInput.error_route_id`
  → `input.errorRoute`.
- The CLI resolves the **nearest ancestor** `error.tsx` (deepest directory wins).
- `makeRealAdapter`'s catch block renders that component with
  `props.error = { message, route, status }`; a broken error.tsx falls back
  to the generic OX-500.
- Wired on **both** the live and prerender paths (the smoke build is
  prerender, which is why the first attempt still emitted OX-500).
- `packages/oxide/test/error-boundary.test.ts` (4 tests) — includes the
  ReferenceError regression guard.
- Smoke app: `routes/boom/page.tsx` throws → `routes/error.tsx` renders with
  the message, both in `dist/prerendered/boom.html` and live.

### 5. route.ts / endpoint dispatch (§8.1, §10.2 step 7) — DONE
- Worker op `endpoint:run`: dispatches named per-method exports (`GET`,
  `POST`, ...) or a default export; requires a Web `Response`.
- `oxide_jshost::EndpointHost` + `oxide_server::EndpointDispatcher` trait +
  CLI `EndpointDispatcherImpl`, wired through `build_dispatch` before the
  render phase.
- `middleware:run` analogous (see below).
- Smoke app: `routes/api/users/route.ts` — GET returns JSON, POST echoes the
  body with 201.

### 6. middleware.ts (§8.1, §8.5) — DONE
- Worker op `middleware:run`: supports `undefined` (pass), `{ redirect }`,
  `{ rewrite }`, `{ headers }`, `{ locals }`, and a Web `Response` (short-circuit).
- `oxide_jshost::MiddlewareHost` + `oxide_server::MiddlewareDispatcher` trait
  + CLI `MiddlewareDispatcherImpl`.
- Runs **before** the loader phase (moved it — the first insertion landed
  after the loader, which made middleware locals unreachable). Ordering now:
  middleware → loader → endpoint → render.
- `locals` threaded: `LoaderRunInput.locals` → `loader_shim::input_to_payload`
  (`"locals"` key) → worker `event.locals`. Two gaps were found and fixed:
  `loader_shim::input_to_payload` was the actual payload builder (not
  `loader_host.rs`), and the shim was dropping both `locals` and `locale`.
- Smoke app: `routes/middleware.ts` injects `locals.user = 'ada'` (loader for
  `/greeting` prints "hello, ada") and short-circuits `?gated=1` with a 307
  to `/login`.

### 7. Defer values (§7.1, §9.2) — PARTIAL (documented)
- `render::SWAPPER_SRC` now includes the boundary's streamed `innerHTML` in
  the `ox:swap` detail as `detail.value` (before it was `{id}` only, so
  every `DeferredClient` resolved to `undefined`).
- **Gap:** `detail.value` is the **rendered HTML** of the boundary, not the
  structured ODF-decoded loader value §9.2 calls for. Closing this requires
  the server to emit the ODF-encoded data alongside the rendered boundary.
  Documented in `SwapDetail.value`'s doc-comment and the defer-glue test.
- Tests: `packages/oxide/test/defer-glue.test.ts` (5 tests, one new for
  `detail.value`).

---

## Still open (honest list)

1. **Loading boundaries (§8.4 "optimistic navigation with loading boundaries").**
   Needs either (a) a client route→component manifest (the
   `RouterOptions.manifest` field is still unused), or (b) real streaming SSR
   in the adapters (all four still `renderToString`). Both are bigger
   infrastructure. This is the only Bucket-2 item not attempted.

2. **Non-React hydration (Vue/Svelte/Solid).** Only React has a real
   `clientAdapter` (`react-dom/client`). The other three alias the neutral-IR
   adapter. Bucket-2 item 7.

3. **Real streaming SSR per framework** — all four adapters still
   `renderToString`. Prerequisite for loading boundaries.

4. **Manifest-driven chunk preloading** — `RouterOptions.manifest` is still
   unused; SPA nav fetches `/__data` and does not preload route chunks.

5. **Middleware `{ rewrite }`** is logged but **not re-matched** in this
   slice. A rewrite returns the original response instead of routing the
   request to the new path.

6. **Middleware locals into actions / route.ts / server-fn.** Locals currently
   reach the *loader* only. Actions and endpoints don't receive them yet.

7. **`goto()` to a route whose loader returns non-200** is still untested
   from the client router.

---

## Verified commands
