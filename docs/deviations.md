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

Vue (`suspense: 'core'`): `packages/vue/src/defer.ts` is exposed at `@oxide-fw/vue/defer` (fixed in 19e8263 — the file existed but was not in the exports map, so no consumer could reach it) and covered by `packages/vue/test/defer-component.test.ts`, which SSR-renders through `vue/server-renderer` and asserts the shell marker + fallback shape against a placeholder value, plus the resolved-inline path against a plain value.

Svelte (`suspense: 'core'`): the `<Defer>` component ships as `packages/svelte/src/Defer.svelte`, exported at `@oxide-fw/svelte/defer` (the export was missing from the commit that first claimed to close this deviation; added in a follow-up), covered by `packages/svelte/test/defer-component.test.ts`. That test compiles the component through `svelte/compiler`, renders it via `svelte/server`, and asserts the placeholder marker + fallback, the resolved-inline path, and the exact-shape check that a wider object carrying `__deferred` alongside other keys is a plain object. This addresses the deviation's own obstacle 2 (Svelte ships `.ts`-only, so a `<Defer>` needs a compiled component): the component ships as source and the consumer's compiler handles it, and the test proves the compiled output emits the marker the swapper expects.

The three obstacles the original entry listed: (1) VNode placeholder markup — solved by the attribute-only swapper selector `[data-ox-b]`; verified by conformance's `streaming: a non-template boundary host swaps too` assertion; (2) Svelte compiled-component requirement — solved by shipping `Defer.svelte` and testing its compiled output; (3) swapper nonce — solved earlier this session, verified in `defer-swapper-nonce.test.ts` per adapter.

An export-map contract test on each adapter (`packages/{svelte,vue}/test/export-map.test.ts`) fails if a shipped component is not reachable — the specific bug class that made this deviation's status momentarily false.

## V2 Wave B foundations — deferred integration points (2026-11-08)

The V2-B09/B02/B01/B05/B06 milestone landed complete in the V8-free crates
(`oxide-fw-router`, `oxide-fw-cache`). The spec wires some of their results
into the CLI/server layer, which links V8 (~5 minutes per iteration on the
reference host, CONVENTIONS §8) — this sandbox has no V8 toolchain budget,
so the following integration points are **deliberately deferred** and must
land with the A01/A02 (cli split + `oxide-dispatch` crate) milestone:

1. **`match_ref` consumers (B02).** The CLI dispatch path still calls
   `match_path` (which now internally uses the candidate index, so it is
   already faster); switching the dispatch closure to `match_ref` +
   `RouteContextTable::get` needs the `DispatchDeps` refactor (B01's
   "Integration" step in spec V2).
2. **`RouteContextTable` at boot (B01).** Built and tested in
   `crates/router/src/context.rs` (oracle-verified against
   `matcher::layout_chain`); `start_server_inner_full` does not construct it
   yet. The acceptance "rg 'for candidate in &tree.entries' crates/cli
   returns nothing" applies to that integration PR.
3. **`KeyStr` in `L0Cache::key` (B05).** `key_of`/`KeyStr` exist and are
   pinned to `canonical()` by `matches_canonical_prefix`; `cli/src/l0_cache.rs`
   still builds 64-char canonical strings. The switch (plus the documented
   `oxide cache clear` migration note, since L0 keys change length) belongs
   to the same integration PR.
4. **Criterion gate CI wiring (B09).** `criterion_summary.py` /
   `criterion_compare.py` exist and the gate was proven to fail a
   deliberately slowed matcher; extending `benchmarks/gate.sh` (or the CI
   `bench-gate` job) to invoke them after the existing table check is left
   as the R03 step. `benchmarks/baseline-v01.json` is committed for it.
5. **`ArchivedTree::match_ref` shape (B02).** The spec materializes
   `Arc<RouteEntry>` per entry at load. This milestone returns
   `ArchivedMatchRef` (winner by index, mapped bytes stay the single source
   of truth) instead: materializing at load would freeze pre-patch bytes and
   break the P-01 liveness contract
   (`archived_tree_reads_through_the_live_mmap`). The index walk falls back
   to the exact linear scan when it cannot complete — the P-01 invariant
   makes this a no-op in production, and it keeps the in-place-patch test
   green.
6. **`dispatch`/`server`/`render` benches (B09).** The spec's bench list
   includes `crates/dispatch/benches/context.rs`,
   `crates/server/benches/metrics.rs` and `crates/render/benches/splice.rs`.
   `dispatch` does not exist until A02; `render`'s `NonceSplicer` lands with
   B08 (needs A01); only `server/benches/metrics.rs` is possible today and
   was deferred to keep the milestone's compile surface V8-free.
