# Deviations from the v0.1 spec

## Per-chunk incremental bundling (5-A)

**What the spec asks.** "Edit one component, one transform + one chunk
re-hash."

**What lands.** An environment-level skip: a per-env fingerprint (excluding
`*.server.*` for the client env) decides whether to re-bundle at all, so a
server-only edit leaves the client bundle valid. Per-route *prerender* reuse
is separate and complete. Per-route chunk hashes are recorded in
`.oxide/graph.json` (`route_chunks`), so "did this route's chunks change?" is
measurable.

**Why not per-chunk.** Rolldown 1.2.13 exposes no incremental/cache API and
no per-chunk write control. The bundle is one `write()` call; there is no
hook between "this module changed" and "re-emit this chunk". A per-chunk
build needs the bundler to publish its module graph and accept a
cache/diff - that is a bundler change, not a CLI change.

**What would change it.** A Rolldown version exposing an incremental cache,
or a switch to a bundler that does. The `route_chunks` records are the diff
target for that work.

## WOFF2 font output (16-B)

**What the spec asks.** WOFF2.

**What lands.** TrueType subset output, CSS `format('truetype')`.

**Why.** No published crate provides a WOFF2 encoder (`woff2` / `woff2-sys`
are not on crates.io; `subsetter` emits TrueType). Hand-writing one means the
WOFF2 glyf/loca transform plus Brotli - a project in itself.

**What would change it.** A published encoder crate.
`crates/assets/tests/font_woff2_deviation_tdd.rs` is the tripwire: it asserts
the `.ttf` extension and the documented deviation, so an encoder swap updates
both deliberately.

## Dev route additions require a dev-server restart (WP-C1) — closed by the dev route rescan

**Status.** Closed. The dev watcher re-scans the route table on a
`Created`/`Removed`/`Modified` event under `routes.dir`, refines the render
modes, and swaps the result into `SwappableRouteTree`. Every request reads
through that slot, so a new route renders without a server restart.
`build_dispatch` wraps a one-shot slot that is never swapped, which keeps
tests deterministic.

**What the spec asked.** Adding a route file under `routes.dir` should render
in dev without editing `oxide.config.ts` or restarting (§13).

**What would have changed it.** A route-table generation in `cmd_dev` that
re-runs `scan_routes` and rebuilds the component map. That is what landed;
the "requires a restart" deviation is retired.

## Edge `mergeHead` was JS-only (P0-1) — closed by `merge_head_structured`

**Status.** Closed. `crates/core-wasm` exports `merge_head_structured`, which
takes and returns the edge head model with the same semantics as the JS:
`title` later-wins, `meta` dedupes by first element keeping the last
occurrence, `links` dedupe by `(rel, href)`, `json_ld` concatenates. Six
tests pin it against the JS behaviour. The edge handler prefers the wasm
export when loaded and falls back to the identical JS.

**Why the old path was wrong.** The Rust `merge_head` merged flat
`Vec<(String, String)>` pairs, but the edge head model is structured
(`{title, meta: [[k,v], ...], links, json_ld}`) with per-kind dedup. The
wasm call silently returned `[]`, so every edge response had an empty head.

**What would have changed it.** A Rust-side port of the structured merge,
exposed as its own export. That is `merge_head_structured`; the flat export
remains a different, intentionally-unrouted API.

## Edge wasm build requires self-hosting layout (P0-1c) — partially closed (payload inert)

**Status.** Partially closed — Rust implementation and payload ship, but the payload is inert. `crates/cli/resources/edge.wasm` is the wasm-bindgen-processed artifact with `__wbindgen_*` imports; the corresponding `edge.js` glue that would satisfy those imports is deleted by `scripts/regen-edge-wasm.sh` (`rm -f .../edge.js`) and not committed. The loader (`crates/adapters/src/lib.rs:905`) does `await import("./edge.wasm")` and expects `mod.default` to be the callable interface — which only the missing glue provides. `useWasm` catches the instantiation error and silently falls back to the semantically-identical JS path.

Effect: an `oxide build` for Cloudflare deploys a payload nobody runs. The JS fallback is correct (BLAKE3 key parity verified earlier); the acceleration is what's missing.

**To unclose:** ship the `.js` glue alongside `edge.wasm` and change the loader to `await import("./edge.js")`. The wasm32-edge CI step `Assert committed wasm is either functional or documented inert` fails until the two cases are reconciled.

## `<Defer>` for `suspense: 'core'` adapters — closed (§7.1)




**Status.** Closed — both `suspense: 'core'` adapters now have a shipped, exported, tested `<Defer>`, and the wire protocol retains its end-to-end coverage from the React `suspense: 'native'` smoke app.

The wire protocol (`<template data-ox-b>` / `<ox-defer data-ox-b>` placeholder, `<template data-ox-c>` + swapper, `ox:swap` dispatch) is verified end-to-end by `tests/playwright/tests/deferred.spec.ts`, which boots `oxide start` on the React smoke app and asserts two deferred settles arrive as distinct resolution scripts in the browser. The protocol is adapter-independent.

Vue (`suspense: 'core'`): `packages/vue/src/defer.ts` is exposed at `@oxide/vue/defer` (fixed in 19e8263 — the file existed but was not in the exports map, so no consumer could reach it) and covered by `packages/vue/test/defer-component.test.ts`, which SSR-renders through `vue/server-renderer` and asserts the shell marker + fallback shape against a placeholder value, plus the resolved-inline path against a plain value.

Svelte (`suspense: 'core'`): the `<Defer>` component ships as `packages/svelte/src/Defer.svelte`, exported at `@oxide/svelte/defer` (the export was missing from the commit that first claimed to close this deviation; added in a follow-up), covered by `packages/svelte/test/defer-component.test.ts`, which compiles the component through `svelte/compiler` and asserts the placeholder marker + fallback, the resolved-inline path, and the exact-shape check. An export-map contract test on each adapter (`packages/{svelte,vue}/test/export-map.test.ts`) now fails if a shipped component is not reachable — the specific bug class that made this deviation's status momentarily false.

The three obstacles the original entry listed: (1) VNode placeholder markup — solved by the attribute-only swapper selector `[data-ox-b]`; verified by conformance's `streaming: a non-template boundary host swaps too` assertion; (2) Svelte compiled-component requirement — solved by shipping `Defer.svelte` and testing its compiled output; (3) swapper nonce — solved earlier this session, verified in `defer-swapper-nonce.test.ts` per adapter.

The wire protocol (`<template data-ox-b>` / `<ox-defer data-ox-b>` placeholder, `<template data-ox-c>` + swapper, `ox:swap` dispatch) is verified end-to-end by `tests/playwright/tests/deferred.spec.ts`, which boots `oxide start` on the React smoke app and asserts two deferred settles arrive as distinct resolution scripts in the browser. The protocol is adapter-independent.

Vue (`suspense: 'core'`): `packages/vue/src/defer.ts` is exported at `@oxide/vue/defer` (this change fixes a real gap — the file existed but was not in the exports map, so no consumer could reach it) and covered by `packages/vue/test/defer-component.test.ts`, which SSR-renders through `vue/server-renderer` and asserts the shell marker + fallback shape against a placeholder value, plus the resolved-inline path against a plain value.

Svelte (`suspense: 'core'`): the `<Defer>` component ships as `packages/svelte/src/Defer.svelte`, exported at `@oxide/svelte/defer`, covered by `packages/svelte/test/defer-component.test.ts`. That test compiles the component through `svelte/compiler`, renders it via `svelte/server`, and asserts the placeholder marker + fallback, the resolved-inline path, and the exact-shape check that a wider object carrying `__deferred` alongside other keys is a plain object. This addresses the deviation's own obstacle 2 (Svelte ships `.ts`-only, so a `<Defer>` needs a compiled component): the component ships as source and the consumer's compiler handles it, and the test proves the compiled output emits the marker the swapper expects.

The three obstacles the original entry listed: (1) VNode placeholder markup — solved by the attribute-only swapper selector `[data-ox-b]`; verified by conformance's `streaming: a non-template boundary host swaps too` assertion; (2) Svelte compiled-component requirement — solved by shipping `Defer.svelte` and testing its compiled output; (3) swapper nonce — solved earlier this session, verified in `defer-swapper-nonce.test.ts` per adapter.
What is verified: the deferred streaming *wire protocol* — `<template data-ox-b>` placeholder, `<template data-ox-c>` + swapper, `ox:swap` dispatch — is tested end-to-end. `tests/playwright/tests/deferred.spec.ts` (added in a13c4e1) boots `oxide start` on the React smoke app and asserts two deferred settles arrive as distinct resolution scripts in the browser. The protocol is adapter-independent, so that test covers the wire contract every `suspense: 'core'` adapter consumes.

What is shipped but untested: the Vue `<Defer>` component (`packages/vue/src/defer.ts`, 63 lines, Vue is `suspense: 'core'`). No test — no Vue smoke app, no conformance import — exercises that component. Its correctness rests on code review, not on a green test.

What is NOT shipped: the Svelte `<Defer>` component. Svelte is `suspense: 'core'` and there is no `packages/svelte/src/defer.ts`; the only `deferred` references in `packages/svelte/src/` are IR node type names, not a `<Defer>` component. This is the deviation's own obstacle 2 — Svelte ships `.ts`-only, so a `<Defer>` component needs either a compiled `.svelte` file or the sentinel-comment design from the WP-A2 plan (keep placeholders out of VNodes entirely; the core HTML pass rewrites `<!--ox-b:ID-->` into `<template data-ox-b>`).

React and Solid are `suspense: 'native'`; `<Defer>` is n/a for them.

So the three obstacles from the original entry are: (1) VNode placeholder markup — **solved** by the attribute-only swapper selector `[data-ox-b]`, verified in conformance; (2) Svelte compiled-component requirement — **open**, no Svelte `<Defer>` shipped; (3) swapper nonce — **solved** earlier in the session. Two of three obstacles are closed; the third needs the Svelte component and, once that lands, a `suspense: 'core'` smoke app + Playwright spec (or a conformance assertion on `<Defer>`-rendered markup per adapter). The Vue component needs the same test treatment to close "untested" to "verified". Both E-effort.
**What lands.** The wire protocol exists and is tested — `template[data-ox-b]`
placeholder, `template[data-ox-c]` + swapper resolution, `ox:swap` dispatch,
error-boundary mapping (`conformance/suite.ts`, four assertions per adapter
against the IR renderer). The IR renderer is its reference implementation.
Vue and Svelte have **no `<Defer>`**; a `deferred(...)` value in a
`suspense: 'core'` real adapter renders its resolved value inline (the
`defer()` fixes below) but never streams.

**Why this is not a small component.** Three obstacles, each found while
attempting the implementation:

1. **Placeholder markup cannot go through VNode rendering.** A Vue
   `<Defer>` that returns `h('template', …)` renders as
   `<template data-ox-b="0"><!--[-->fallback<!--]--></template>` — Vue's
   hydration comment markers land inside the template, and the swapper
   moves `t.innerHTML` into the page, so the markers end up in the DOM.
   The placeholder must be emitted as raw HTML, which needs a render
   escape hatch (a `v-html` wrapper, or an adapter that post-processes a
   sentinel).

2. **Svelte needs a compiled component.** The package ships `.ts` only
   (`src/Image.svelte` is the one exception, declared in `exports`).
   `<Defer>` would have to be a `.svelte` file with its own build step, or
   a string-template rendered through `svelte/server` — a different
   mechanism from Vue's, which means the "one `<Defer>` per framework" in
   the spec is genuinely two implementations, not one shared one.

3. ~~**The swapper `<script>` has no nonce.**~~ **FIXED** — the nonce now
   travels `PageRenderInput.nonce` → `FrameworkRenderInput.nonce` →
   `RenderInput.nonce`, and each IR renderer passes it to
   `boundaryContent`. Both call sites populate it: the live path with
   `nonce_for(request_id)`, the prerender path with the build-time nonce it
   already stamps on its own injected tags. Verified end-to-end — the
   emitted swapper carries `nonce='…'`, and it is omitted when the
   deployment configures no CSP. This was a pre-existing bug in the IR
   path, independent of `<Defer>`, and would have blocked any streaming
   adapter.

**What was tried and reverted.** A render-scoped registry
(`createBoundaryRegistry` / `withBoundaryRegistry` / `registerBoundary` /
`deferredIdOf`) plus a Vue `<Defer>` emitting `h('template', …)`. Reverted
on hitting obstacle 1 — the emitted markup was wrong in a way that would
have shipped broken boundaries.

**What would change it.** Solve obstacle 1 first (a raw-HTML escape hatch
the adapter can use), then obstacle 2 (Svelte needs a compiled component,
so its `<Defer>` is a separate implementation). Obstacle 3 is fixed; the
nonce now reaches every streaming render.
