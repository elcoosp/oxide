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

## V2-A02 dispatch substrate — deferred CLI adoption (2026-11-08)

The V2-A02 milestone landed the V8-free `oxide-fw-dispatch` crate complete
per the spec's acceptance (`cargo test -p oxide-fw-dispatch` in 1.3 s;
`cargo tree -p oxide-fw-dispatch | grep -c jshost` = 0). What is
deliberately deferred, and why:

1. **The CLI still owns duplicate copies of the moved helpers.** The spec's
   "move, do not rewrite" applies to the phase-3 PR that splits
   `build_dispatch` (V2-A01 phase 3), because deleting the CLI's private
   `parse_form_urlencoded` / `action_from_query` and re-pointing the closure
   at `oxide_fw_dispatch::url` requires compiling `oxide-fw-cli` — a V8 link
   (~5 min per iteration, CONVENTIONS §8) this sandbox cannot pay. Until
   then both copies exist; the dispatch copies are the tested, `pub`,
   `#[must_use]` ones and the tests pin byte-for-byte semantics (including
   the `"//"` and empty-input edge pins that document current closure
   behaviour).
2. **`chain.rs` / `loads.rs` / `deps.rs` / `ctx.rs` are absent.** The spec
   assigns them to V2-A05, V2-A06 and the A01 phase-3 move respectively;
   landing them now would mean inventing CLI-private state (`DispatchDeps`
   captures `l0`, `pool`, `waterfall`, …) without a compile to check
   against.
3. **`[workspace.lints]` is opt-in, not workspace-wide.** The policy table
   now exists at the root (`rust.warnings = deny`,
   `clippy.pedantic = warn, priority = -1`), but only `oxide-fw-dispatch`
   declares `[lints] workspace = true`. Rolling it across the other 18
   crates is a dedicated hygiene pass (the existing crates were never held
   to pedantic; `--no-deps`-scoped runs on `oxide-fw-diagnostics` alone
   surface 13 pedantic diagnostics). The command-line pedantic gate for the
   new crate is therefore run as
   `cargo clippy --no-deps -p oxide-fw-dispatch --all-targets
   --all-features -- -D warnings -W clippy::pedantic`.
4. **The dispatch bench ids are not in the criterion baseline.**
   `baseline-v01.json` / `after-wave-b.json` cover the Wave B ids only;
   registering the new `url/*` and `ctx_table/*` ids belongs to the same
   R03 gate-wiring step already deferred in "V2 Wave B foundations".

**To unclose:** land A01 phase 3 on a V8-capable host; it deletes the CLI
duplicates, flips the closure to the stage protocol, and switches the
helpers to `oxide_fw_dispatch::url::*` in the same commit series.

## V2-A06 parallel load graph — deferred CLI adoption, pedantic adoption scope (2026-11-08)

The V2-A06 milestone landed both spec steps V8-free (token-exact
`needs_parent` for pages AND layouts in `oxide-fw-router`; the
`run_load_graph` substrate in `oxide-fw-dispatch::loads`). What is
deliberately deferred, and why:

1. **The CLI's `stage_loaders` still runs the sequential loop.** Switching
   the dispatch closure to `run_load_graph` + the
   `RouteContextTable.layout_loads[i].needs_parent` verdicts requires
   compiling `oxide-fw-cli` (a V8 link, CONVENTIONS §8) — impossible in
   this sandbox. The substrate is complete and pinned from outside the
   crate (12 integration tests, paused-clock timing pins); the wiring is
   code-only once A01 phase 3 can compile the CLI. Until then the dispatch
   comment naming the gap stays accurate, and the bench pair
   `loads/parallel_8` (2.10 ms) vs `loads/chain_8` (16.60 ms) documents
   exactly what the wiring buys.
2. **The pedantic lint policy now covers `router` and `schema`, not yet
   the whole workspace.** A02 introduced `[workspace.lints]` with
   `oxide-fw-dispatch` opted in; this session fixed 107 pedantic sites and
   opted in `oxide-fw-router` + `oxide-fw-schema` (the crates A06
   substantively touches). The remaining V8-free crates (cache, odf,
   config, graph, diagnostics, adapters, plugin, render, server, dev) are
   still held to plain `-D warnings` (verified clean over the 13-crate
   graph) and adopt pedantic in a dedicated hygiene pass; the V8 crates
   (cli, jshost, bundler, compiler, assets) cannot be linted here at all.
   Zero `#[allow(clippy::*)]` exists anywhere; the two long functions
   (`walk_dir`, `typegen_impl`) were split into helpers rather than
   suppressing `too_many_lines`.
3. **`loads/*` bench ids are informational.** As with the A02 ids,
   `criterion_compare.py` only gates ids present in BOTH snapshots; the
   new group joins the baseline at the next promotion (R03, deferred).

**To unclose:** A01 phase 3 on a V8-capable host deletes the CLI's
sequential loop in favor of `run_load_graph`, adopts the layout verdicts,
and runs the remaining-crate pedantic hygiene pass in the same series.
