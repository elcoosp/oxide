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

## Edge wasm build requires self-hosting layout (P0-1c) — closed by embedded `edge.wasm`

**Status.** Closed in `e616831`. `oxide-core-wasm` is built to
`wasm32-unknown-unknown`, processed with `wasm-bindgen-cli` 0.2.129, embedded
in the `oxide-cli` binary via `include_bytes!`, and written to
`<dist>/edge.wasm` for every project. A user project no longer needs to be a
cargo workspace. `scripts/regen-edge-wasm.sh` regenerates the checked-in
artifact when the `#[wasm_bindgen]` surface changes; the CI job `wasm32-edge`
keeps the compile path honest.

**Why embedding rather than `build.rs`.** A `build.rs` would make every
`cargo build -p oxide-cli` pay the wasm32 + wasm-bindgen cost even when the
user never targets Cloudflare. The checked-in artifact is a build-time
invariant; refreshing it is an explicit operation.

**What the gate caught.** The wasm target had two real type errors hidden by
`#[cfg(target_arch = "wasm32")]` (an ODF/serde type mismatch in
`encode_odf`/`decode_odf`) and a host-only rustflag that `rust-lld` rejected.
Both are fixed, and a non-ignored test asserts the embedded bytes are written
for a root that is not a cargo workspace.

## `<Defer>` for `suspense: 'core'` adapters is designed, not shipped (§7.1)

**What the spec asks.** §7.1: "Adapters with `suspense: 'native'` use their
own mechanism; adapters with `suspense: 'core'` get boundaries from a
`<Defer>` component the core exports per framework (generated from the
adapter)."

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
