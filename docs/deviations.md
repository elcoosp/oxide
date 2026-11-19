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

1. **`match_ref` consumers (B02).** *(CLOSED since — 2026-11-16: the
   dispatch match sites go through the unified `MatchedView` seam — see
   "V2-B01 + B02 unified match view adoption" below.)* The CLI dispatch path
   still calls `match_path` (which now internally uses the candidate index,
   so it is already faster); switching the dispatch closure to `match_ref` +
   `RouteContextTable::get` needs the `DispatchDeps` refactor (B01's
   "Integration" step in spec V2).
2. **`RouteContextTable` at boot (B01).** *(CLOSED since — 2026-11-16: the
   table rides the `SwappableRouteTree` generation, rebuilt on every dev
   rescan — see the adoption entry below; the build-time prerender scans
   remain a queued micro-follow-up.)* Built and tested in
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
   against. *(Status: partially closed since — `loads.rs` landed with
   V2-A06 and `chain.rs` + `locals.rs` landed with V2-A05; `deps.rs` /
   `ctx.rs` remain A01-phase-3 material because they capture CLI-private
   state.)*
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

*(Status: CLOSED since — the CLI adoption landed 2026-11-16, see "V2-A05 + A06 CLI stage adoption" below.)*

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

## V2-A05 middleware chain + unified locals — deferred CLI adoption (2026-11-08)

*(Status: the chain half is CLOSED since — `stage_middleware` walks the chain through the substrate since 2026-11-16, see "V2-A05 + A06 CLI stage adoption" below; the `RequestLocals`-in-`stage_prelude` half remains queued.)*

The V2-A05 milestone landed the spec's step 1 complete and V8-free:
`oxide-fw-dispatch::chain` (`run_chain`, the §8.5 chain runner with the
production semantics — later-wins locals, last-writer-wins headers per
case-insensitive name, short-circuit / redirect stop the walk, rewrite
returns the target with the B-22 resume marker) and
`oxide-fw-dispatch::locals` (`RequestLocals` with the pipeline-owned
reserved keys `session` / `locale` / `experiments` and the DSP-101
collision report). 22 integration tests pin both from outside the crate.
What is deliberately deferred, and why:

1. **The CLI's `stage_middleware` keeps its inline loop.** Replacing the
   ~200-line closure body with `run_chain(.., &ctx.middleware_chain, ..)`
   requires compiling `oxide-fw-cli` — a V8 link (CONVENTIONS §8) this
   sandbox cannot pay, exactly as with the A02 helpers and the A06 load
   graph. The wiring is code-only once A01 phase 3 can compile the CLI;
   the substrate already mirrors the loop's behaviour (the CLI's own
   `middleware_chain_runs_outer_to_inner_and_threads_locals` test pins the
   end-to-end shape the substrate matches).
2. **`stage_prelude` does not build `RequestLocals` yet.** The session
   decode, locale resolution and experiments staging stay in the closure;
   `to_value()`'s reserved-key precedence and the DSP-101 report are
   pinned but not yet consumed by loaders/actions/render.
3. **`/__data` middleware parity is unchanged.** The §8.4 parity the spec
   asks for in step 3 (the data endpoint running the same chain) is a
   property of the closure wiring, not the substrate; today's behaviour
   (the data branch runs after the middleware phase with the same
   locals/headers) is preserved verbatim by the deferral.
4. **Spec-sketch deltas are deliberate.** The substrate's signature
   follows the real `MiddlewareDispatcher::dispatch(MiddlewareDispatchArgs)`
   seam (the spec's positional sketch predates the args struct), and the
   rewrite outcome carries a resume marker because the CLI's B-22 fix
   postdates the spec text. `ChainOutcome::Respond` keeps the raw
   `MiddlewareRunResult` so the caller's session write-back and location
   -header mapping stay where they are today.

**To unclose:** A01 phase 3 on a V8-capable host replaces the loop body
with `run_chain`, builds `RequestLocals` in `stage_prelude`, and deletes
the closure's ad-hoc locals special-casing in the same series.

## V2-A05 + A06 CLI stage adoption — the stage-tree wiring (2026-11-16)

The A01 phase-3 stage tree made the CLI dispatch closure compilable in this
sandbox (`cargo check --all-targets` links no binaries), so the two deferred
substrate adoptions landed as the M1 milestone:

1. **`stage_middleware` walks the chain through the substrate.** The ~200-line
   inline dispatcher loop became
   `oxide_fw_dispatch::chain::run_chain(..)` over `cx.middleware_module_paths`
   (the P-06 chain, unchanged), with `ChainRequest` carrying the request-shaped
   fields and the stage keeping what the substrate leaves to the caller: the
   §16.6 session decode into the initial locals, the B-22 resume marker
   handling (the substrate filters a marker naming an absent layer), the
   response mapping for `ChainOutcome::Respond` (location header, the B-05
   short-circuit session write-back), the §8.5 rewrite re-dispatch through
   `self_dispatch` with the hop bump, the error envelope, the post-chain
   B-05 write-back and the F15 record capture (shared `finish_middleware_stage`
   postlude so the three paths cannot drift). Two deliberate deltas, both
   substrate-authoritative: a `{ redirect: …, short: false }` answer now stops
   the chain and is honored (the spec's `redirect_stops_chain`; the old loop
   ignored it unless `short` rode along), and when `self_dispatch` is missing
   (unreachable in production — the wrapper publishes before the first
   request) the rewrite target is dropped and the accumulated state serves,
   instead of the old code silently running the remaining layers.

2. **`stage_loaders` runs the §9.2 load graph.** The sequential layout loop
   and the (last-layout, page) `tokio::join!` pair became ONE
   `run_load_graph_outcomes` call: a `LoadNode` per layout with its OWN
   scanner verdict (`LayoutLoadSpec` carries `needs_parent` from the A06
   step-1 token-exact scan — a layout that calls `parent()` serializes behind
   its predecessor, one that does not runs concurrently), plus the page node
   when the route has a server module. The page node runs through the
   runner's streaming entry (B2 settle streaming preserved — the graph's new
   `streaming` index set hands the receiver back via `GraphOutcomes`);
   `GraphOutcomes` also carries the per-node elapsed column (the node's own
   runner call, parent wait excluded) and the failing node's index + full
   diagnostic (`LoadResult::Failed { index, code, message }` — the CLI keeps
   its layout-vs-page error bodies and the real diagnostic code in the log).
   Side effects accumulate outer-to-inner in input order and the page
   OVERWRITES the accumulated cookies/headers — the preexisting overwrite
   semantics, byte-identical. The waterfall's `overlap-saved` mark
   generalizes from `min(layout, page)` to `sum(elapsed) − wall` (for the
   pre-A06 two-node join the two formulas agree exactly); the superseded
   `parallel_load_savings_ms` helper and its pins are deleted with it. The
   /__data path keeps its sequential chain and reads only the spec's
   id/path pair (parallelizing it is a follow-up).

**Test placement (the V2.0 item-3 rule):** the substrate deltas are pinned
V8-free in `crates/dispatch/tests/loads_tdd.rs` (12 → 17 pins: the settle
receiver by index, streaming+parent composition, the elapsed column in input
order, outcomes-complete-on-short-circuit, the failed index/code, the
delegation) and ran green here; the pipeline pins
(`load_graph_threads_layout_data_to_a_parent_calling_page`,
`independent_layout_loads_overlap_with_the_page_under_the_load_graph` — the
two-layout three-way overlap the join pair could not express) live in
`crates/cli/src/tests.rs`, compile-verified locally and riding the CI
`cargo nextest run --workspace` job (the cli crate is V8-linked). The
pre-existing pins — `layout_and_page_overlap_*`, `middleware_chain_runs_*`,
`middleware_rewrite_does_not_rerun_outer_layers` — stay byte-identical and
pin the adoption's behavior contract.

**Gate-script drift fixed (additive refinements):** `split_selfcheck.py`
was broken at HEAD twice over — `stages_check` read
`HEAD:crates/cli/src/dispatch/entry.rs`, which the phase-3 runner commit
(2a77c967) had already reduced to the 244-line stage runner, so the recorded
SPANS (against the 2,836-line pre-split closure) IndexError'd; and
`phases_check` read `HEAD:crates/cli/src/build/cmd.rs`, which the phase-2
orchestrator commit (d4400e5e) had already reduced to 41 lines, failing the
recorded PHASE_SUBS asserts. Both now pin the last revision where the
pre-split text still lived (`2e0a13bc` for stages, `aa0159ac` for phases) —
the proofs are GREEN again (52,183 stage tokens and 2,335 phase lines vs
HEAD, zero diffs). The M1 adoption itself is recorded the established way:
`STAGE_ADOPTED` marker-checks for `middleware`/`loaders` (require the
substrate call, absent the superseded loops) and a recorded orig-side
substitution for `stage_route`'s `LayoutLoadSpec` verdict carry — the other
eight stages stay token-checked verbatim.

## V2-B01 + B02 unified match view adoption — the dispatch match path (2026-11-16)

The A01 phase-3 seam made both Wave-B integration points contained; they
landed as the M2 milestone:

1. **`RouteContextTable` at boot (B01).** The table rides the tree
   generation: `SwappableRouteTree` builds it in `new` (`build_with_default_pattern`
   — the crate-default pattern function matches the CLI's `pattern_of`
   exactly), rebuilds it in `store` (a dev rescan never serves stale
   per-route answers), and `load_pair` hands the dispatch the tree and its
   table under ONE read guard — a swap between the two loads cannot mix
   generations, so a matched route id is always a table hit. The prelude
   stores the pair on the request context; `stage_match` resolves
   `route_ctx` (matched id → `table.get`, unmatched → `table.unmatched`)
   and the metrics label becomes the precomputed `rc.pattern` (the
   per-request `pattern_of` walk stays only as the degenerate fallback).
   `stage_route` reads every chain answer from the context: the middleware
   chain (`rc.middleware_chain`, routes-root joined), the not-found
   route + chain (`table.unmatched()`), the innermost error boundary
   (`rc.error_route`), the layout chain (`rc.layout_chain`), and the layout
   load specs (`rc.layout_loads` — the A06 `parent()` verdict rides the
   same row, so the per-layout `is_file()` **stat per request is gone**;
   the table is flag-driven on `has_server_module`). The tree's P-06
   indexes stay as the degenerate fallbacks on the `None` arms (a matched
   route always hits; the fallbacks cover an unmatched URL on a tree with
   no not-found entry, where the old code answered the root chain).

2. **The unified match view (B02).** `crates/router/src/view.rs`:
   `MatchedView` — the one seam over `MatchRef` (borrowed entry) and
   `ArchivedMatchRef` (index into the mmap, per the P-01 liveness
   deviation) — with in-place accessors (`route_id`, `score`, `params`,
   `is_live_route`, `layout_chain`) and `into_match_result` (the
   byte-identical owned shape `match_path` returns). Adoption: `stage_match`
   matches through the seam on both trees (the candidate index hot path,
   owned result only at the end) and the prelude's F05 live gate reads
   `is_live_route` with NO materialization (it used to build a full owned
   `MatchResult` — entry clone + params map — to answer
   `route.live.is_some()`).

**Deliberately queued (recorded honestly):** the build-time prerender scans
(`build/prerender.rs` — the layout-chain/layout-modules/error finds) are
per-route-per-BUILD, not the per-request walk the B01 evidence targets;
adopting them there needs the group-route divergence resolved first (the
scan matches wraps against the CANONICAL id, the table's prefix walk uses
the raw id — the table is the matcher-oracle, so adopting it CHANGES
prerender layout discovery for group routes and with it the A01 Step-0
build digests). Queued as the follow-up with the digest re-measure.

**Test placement:** the view is pinned V8-free in `view.rs` (4 tests:
live-arm oracle equality over 8 paths incl. catch-all/param/escaped shapes,
archived-arm oracle equality over the same paths through a real archived
mmap, the live-field read on both arms, params parity incl. the owned
catch-all join) — all green here; the router crate goes 79 → 83 tests. The
pipeline pins ride the CLI's CI job (V8-linked): the existing
`middleware_*`/`layout_and_page_*` pins keep the behavior contract, and the
two new M1 pins exercise the graph through the table-sourced verdicts.
## V2-E12 workspace links — dev-boot adoption (2026-11-16)

`crates/cli/src/dev/cmd.rs`'s watch-set boot replaces `Watcher::with_notify(roots)`
with `Watcher::with_workspace_links(roots, &ManifestWorkspaceLinks, &rootp)` — the
linked packages the app imports resolve to their real trees and join the watch
set. Deliberate delta: a malformed workspace manifest is a LOUD `DEV-WS-001`
warn + roots-only boot rather than a failed dev boot — the seam's "never a
silent roots-only boot" contract is met by naming the failure; dev keeps
working. The seam's own pins (`crates/dev/tests/workspace_tdd.rs`) ran green
in-crate; the boot call-site is compile-verified (V8-linked) and rides the CI
nextest job. The remaining E12 half (config-side workspace declarations, the
`config` crate surface) stays queued.
## V2-F04 experiments substrate — spec pseudocode deltas and deferred wiring

**Status.** Substrate landed (`crates/dispatch/src/experiments.rs`,
`crates/dispatch/src/locals.rs::set_experiments`, `crates/config`
`experiments` section + DSP-110 load validation + JSON schema). This is the
V8-free step 1 of §V2-F04 (cache-safe personalization); it follows the
A02/A05/A06 pattern: substrate + outside-the-crate pins first, CLI wiring
deferred.

**Deliberate deltas from the spec pseudocode:**

1. **`Resolved.set_cookie` is a `Vec<String>`, not `Option<String>`.** The
   spec's single slot silently drops every minted cookie except the last
   when two cookie-sticky experiments both need a fresh identity for one
   request — and two `Set-Cookie` values can never legally share one header
   (each needs its own header line). The wiring appends one header per
   minted identity after the L0 lookup and strips them from the cached
   entry, which satisfies the spec's intent (cookie identities in
   different buckets see their own variant) for the many-experiment case.
   Pinned by `two_cookie_experiments_each_get_their_own_minted_cookie`.
2. **The suffix join is `;` with no space** (spec-verbatim
   `join(";")`): the suffix feeds an L0 cache key (`|exp:` + suffix), so
   the exact bytes are pinned by `vary_suffix_lists_assignments_id_sorted`
   — `id=variant` pairs, id-sorted (BTreeMap), byte-stable across config
   orderings.

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

1. **`stage_experiments` is not wired into the CLI pipeline** — the
   `resolve(..)` call between `stage_prelude` and the L0 lookup, the
   `|exp:` + `vary_suffix` L0-key dimension in `l0_cache.rs`, and the
   `set-cookie` strip in `make_entry` all live in V8-linked code. The
   substrate's acceptance contract ("with no `experiments` configured, L0
   keys and hit rate equal the pre-F04 baseline") holds trivially at the
   substrate level: `Resolved::vary_suffix` is `""` without experiments,
   and the wiring appends a key dimension only when it is non-empty.
2. **Metrics (`oxide_experiment_assignment{experiment,variant}`) and the
   `oxide why <route>` key breakdown** are CLI/server surfaces, deferred
   with the stage wiring.
3. **Config → engine conversion is wiring code**, not a dependency: the
   config crate does not (and must not) depend on dispatch, so
   `ExperimentConfig` → `experiments::Experiment` is a field-by-field
   mapping at the stage site; both shapes are pinned by their own crates.

**Also landed with this session (hygiene debt retired):** `oxide-fw-config`
adopted the workspace lints policy (`[lints] workspace = true`) and its
sources are pedantic-clean with zero `#[allow]`; the 213-line
`config_schema()` json! literal was split into per-section helpers (the
`scripts/split_cli.py`-style codemod `split_schema.py` — the JSON bytes are
unchanged and the stability test still pins them), and the `size.rs`
percentage math avoids the flagged float→int casts with an exact
decimal round-trip that preserves the old truncating/saturating semantics.

**To unclose:** A01 phase 3 on a V8-capable host wires `stage_experiments`
between prelude and the L0 lookup, threads `Resolved::assigned` into
`RequestLocals` via the already-landed `set_experiments`, appends the vary
suffix to the L0 key, strips minted cookies from cached entries, and adds
the `oxide_experiment_assignment` counters.

## V2-A04 build id + generations substrate — spec deltas and deferred wiring

**Status.** Substrate landed (`crates/cache/src/lib.rs::build_id_of_dist`
+ `stamp_build_id`/`read_build_id`, `crates/dispatch/src/build.rs` —
`GenerationSet`, `stage_build_check`, `advertise_build`,
`is_data_plane_path` — plus the additive
`render::hydration_bootstrap_with_build` / `Injector::with_build_id` and the
`packages/core/src/client/build.ts` client helpers wired into the router,
`enhance` and `__oxide_call_fn`) AND the CLI wiring landed
(`feat/a04-build-identity-wire`: the build's canonical stamp, the
dispatch wrapper's stage install + advertise, the streamed-shell
bootstrap field, the `#[ignore]` e2e pin + CI step — see the deltas
below). This follows the A02/A05/A06/F04 pattern: the V8-free surface
ships complete and pinned, the V8-linked wiring rides the same-session
milestones that can compile it.

**Deliberate deltas from the spec text:**

1. **`hydration_bootstrap` keeps its three-argument signature; the build
   field ships as an additive overload.** The spec's "update the signature
   to `hydration_bootstrap(odf_b64, route_id, nonce, build_id)`" assumed A01
   phase 3 had already moved the CLI call sites (`crates/cli/src/lib.rs`
   still calls the 3-arg form from V8-linked code this sandbox cannot
   compile). Breaking that signature now would leave the workspace
   un-compilable on a V8-capable host. `hydration_bootstrap_with_build` is
   the four-field variant; `Injector::with_build_id` opts a page into it,
   and the 3-arg shape is byte-identical to the pre-A04 output (pinned by
   `legacy_bootstrap_has_no_build_field`).
2. **`stage_build_check` takes the raw header list and an `Instant`.** The
   spec's sketch lives inside the dispatch closure's `RequestCtx`, which is
   A01-phase-3 material (it captures CLI-private state). The substrate's
   signature is the pure shape of the same decision — path, headers,
   generation set, now — and the wiring adapts it 1:1.
3. **`is_data_plane_path` covers only today's data plane** (`/__data`,
   `/_oxide/fn/*`). The spec's "later `/__oxide/live`, `/__oxide/stream/*`"
   routes do not exist yet; adding them to the set before the routes land
   would enforce a header no client sends. The pins reject
   `/__oxide/stream/*` explicitly so the routes' WPs flip the predicate
   deliberately.
4. **`build_id_of_dist` tolerates missing covered inputs.** A missing
   `.oxide/routes.manifest` or `dist/server/**` narrows the manifest to
   what exists instead of erroring; only a missing `dist` root errors
   (`CAC-BUILDID`). The wiring runs at the end of `phase_emit`, on whatever
   the emit phase actually produced, and a narrower manifest still changes
   the id (so a half-emitted build is never accepted as its predecessor).
5. **`handleStale`'s reload default is `globalThis.location?.reload()`.**
   The spec's `location.reload()` default throws in the Node test runner
   (`packages/core` tests on the built-in runner, no jsdom); optional
   chaining makes the module import-safe in the SSR/config-loader Node
   environment (the `boot.ts` contract) with identical browser behaviour.
   The storage guard treats a missing/blocked `sessionStorage` as "reload
   once" — the server's 409 is authoritative and no-guard beats
   stuck-stale.
6. **`stage_build_check` runs before the L0 lookup in the wiring's plan,
   not in this substrate.** The dispatch closure is V8-linked; the
   substrate stage composes with the `Flow` protocol the phase-3 stages
   return, so the wiring is a call site, not a redesign.

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

1. ~~`phase_emit` does not compute or persist the id yet~~ — **LANDED**
   (this diff, `feat/a04-build-identity-wire`): `cmd_build`'s emit tail
   calls `oxide_fw_cache::stamp_build_id(&dist)` after the LAST covered
   write (the i18n manifest stamp), writing `dist/build-id` +
   `.oxide/build-id` atomically and printing the id in the build summary.
   `read_build_id` is the one read site; the boot's build-id chain gained
   the stamp as its second source (manifest field → stamped id → `"dev"`).
2. ~~No stage in the pipeline~~ — **LANDED**: `build_dispatch_swappable`
   wraps the inner dispatch — an unaccepted `x-oxide-build` on the data
   plane answers `409` + `x-oxide-build-stale: true` before route matching
   or the L0 lookup, every continued data-plane response carries
   `x-oxide-build: <current>` (`advertise_build`, idempotent), and the
   streamed shell's hydration bootstrap bakes the CURRENT id via
   `hydration_bootstrap_with_build` (the same value the stage checks, so
   the client echo is accepted). Non-data-plane requests and the test
   helper's `None` handle pass byte-identical to pre-A04.
3. **No generation promotion yet** — the boot constructs the
   `GenerationSet` (the boot id accepted, the window from the typed
   `deploy.skewWindow`) and `StartedServer` carries the handle, but the
   `cmd_dev` reload promote/expiry is the F10 slice (this session's M2).
4. **The conformance assertion "build id advertised"** (HTML carries
   `build:`, `/__data` answers carry `x-oxide-build`) — covered on the
   wire by the `#[ignore]` e2e pin
   `build_identity_advertises_and_rejects_stale_on_the_wire` (+ its
   `jshost-e2e` CI step); the conformance-suite assertion remains with the
   C25/C26 rows.

**New deltas from the wiring (this diff):**

1. ~~Prerendered shells keep the legacy three-field bootstrap~~ — **CLOSED**
   (this diff, `feat/live-loaders-wire`, 2026-11-15): the emit tail now
   bakes the STAMPED id into every prerendered shell AFTER
   `stamp_build_id` computed it and with no covered writer left to run
   (`crates/render/src/lib.rs::bake_build_id`, called from `cmd_build`'s
   tail over `artifact.prerendered`). The bake is byte-surgery on the
   bootstrap's `window.__oxide={route:"…"` marker — the four-field A04
   shape, idempotent under repeated builds, re-baking a stale id on an
   incremental rebuild — and the shells live at `dist/<page>.html`,
   OUTSIDE the covered set (`dist/client/**`, `dist/server/**`,
   `.oxide/routes.manifest`), so the baked bytes never re-stamp the
   identity: the id a shell echoes is by construction the id the stage
   accepts, no 409 loop. The prerender Injector still keeps `build_id:
   None` (the pass runs pre-tail); the bake is the post-tail point the
   original delta reserved. Pinned V8-free in
   `crates/render/tests/bake_build_id_tdd.rs` (5 pins: the byte-exact
   A04 shape, idempotence, stale-id re-bake, multi-marker + no-marker
   no-op, id sanitization) and over the wire by the extended
   `build_identity_advertises_and_rejects_stale_on_the_wire` e2e (the
   shell carries the stamped id — its `jshost-e2e` CI step runs it).
2. **The stamp writes both files atomically** (temp + rename in the
   target's own directory — rename across filesystems fails) and neither
   stamped file sits inside the covered set, so re-stamping an unchanged
   dist yields the same id (the spec's "build twice, same id" proof holds;
   pinned in `build_id_tdd.rs`).
3. **The boot's build-id read order is manifest field first.** The L0
   seed keys were computed with the bundler's manifest-field id at build
   time; the boot reads the field first (seed agreement), the stamped
   canonical id second (a dist whose manifest predates the field),
   `"dev"` last.

**Hygiene rides-along (recorded because the gates demand them):** the
workspace is now free of `#[allow(...)]` attributes — the pre-existing sites
in `graph`/`odf`/`server` test helpers were turned into real tests or
renamed to `snake_case`, the stale `dead_code` pair in the CLI's
`tests.rs` (callers exist unconditionally) and the unreachable tail block in
`jshost/src/loader_host.rs` (both match arms return) were deleted, the
compiler's `AstBTreeMap` import+alias shim is gone, and the bundler doc
comment no longer spells an allow attribute. The CLI and jshost edits are
verified by reading (both crates link V8 and cannot compile in this
sandbox); the graph/odf/server/compiler/bundler edits are test-verified.
Also cleaned: `.gitignore` carried pasted shell output (`=== HEAD ===` …)
from an earlier session and now ignores the machine-local
`.github/pr-body.md`. Docs-drift note: spec A03 item 1 claims the CI push
branch filter is the truncated token (`ain]`) — the tree already has the
correct filter; the spec text predates the fix and the item is closed in
the tree.

## V2-F05 live loaders — deltas and deferrals (wired)

**Status.** Substrate landed (`crates/server/src/live.rs`,
`crates/router/src/lib.rs::RouteLive` + scan,
`crates/config/src/lib.rs::LiveConfig`,
`crates/dispatch/src/build.rs` live data-plane contract,
`crates/render/src/lib.rs` bootstrap,
`crates/adapters/src/lib.rs::Capability::Live`,
`packages/core/src/client/live.ts`). **The wiring landed**
(`feat/live-loaders-wire`, 2026-11-15):

- the `GET /__oxide/live` endpoint arm in the dispatch closure
  (`crates/cli/src/lib.rs`): `same_origin` gate (`403`; missing evidence
  denies), the live-route lookup (`404` otherwise), `parse_live_query`,
  subscribe-or-`503` via `live_endpoint_response` (the body holds the
  subscription `Guard` — an ended/disconnected body unsubscribes), the
  connect-time first snapshot (enqueue → rerun → push);
- the runner spawned once per boot (`crates/cli/src/serve/live.rs::LiveState`
  + `spawn_runner`) with the rerun closure dispatching the SAME data-route
  path as `/__data` (`rerun_request` carries the stored header snapshot);
  a redirect/error/empty result is the stale signal —
  `live_stream_with_stale` answers `event: stale` + the current build id
  and ends the stream;
- the revalidation hooks (§V2-F05 step 6) on the action's
  `revalidate.tag`/`revalidate.path` events: `notify_tag`/`notify_path`
  fan out through `Hub::affected` (`path` events compute the `path_tag`
  convention);
- the client lifecycle (`packages/core/src/client/boot.ts`):
  `connectLive` is imported and driven from the wrapped `adapter.update` —
  every navigation syncs the connection with the current input's route +
  `live` flag; the hydration bootstrap's flag opens it at boot; the boot
  disposer closes it. The `/__data` envelope advertises `live: true` for
  live routes (`normalizeInput` carries it into `ClientInput.live`);
  non-live envelopes stay byte-identical.

**Deliberate deltas from the spec text (refinements, not forks):**

1. **`Sub.tags` is a `BTreeSet`, not the spec's `HashSet`** — `affected`
   fan-out order must be deterministic for the same tag set (pins, stable
   SSE ordering); the hub indexes are `DashMap` either way.
2. **`LiveHub::new(max, per_ip_max)` and `Result<Guard, SubscribeError>`**
   instead of the spec's `new(max)` + `Result<Guard, ()>` — the config
   surface carries `live.perIpMax` (the spec's own security bullet
   "per-IP subscriber cap (default 20)"), and the endpoint needs to know
   which limit tripped for its 503; the error type names the two.
   `subscribe` keeps the spec's fetch-add-then-rollback capacity check.
3. **The runner's debounce window is per received batch**, not a
   per-subscriber timer wheel — with one notification stream a batch
   window collapses the same burst into one rerun (the spec's own test:
   "debounce collapses 5 notifications in 50 ms into one rerun" passes
   verbatim under paused time); the per-subscriber rate cap carries the
   per-connection bound (`maxRerunsPerSecond`).
4. **`same_origin` treats missing evidence as deny** — `Sec-Fetch-Site`
   present and not `same-origin` → deny; no `Origin`/no `Host` → deny.
   The wiring supplies the headers verbatim; proxies that strip
   `Sec-Fetch-Site` fall to the Origin-vs-Host authority compare (host
   case-insensitive, port exact, scheme-default port for a port-less
   Host header).
5. **`live_stream` payloads are base64-encoded by the substrate** (the
   spec's `data: <base64 ODF>` line) — the `watch` channel holds raw ODF
   bytes; encoding at the frame boundary keeps the SSE line framing
   byte-safe and the rerun closure payload-agnostic.
6. **`connectLive` is transport/document/reload-injectable** — the Node
   test runner (`node --test`, no jsdom) cannot construct an
   `EventSource` or `document`; injection keeps the module import-safe
   (the `boot.ts` contract) with the spec's globals as defaults.

**What remains deliberately deferred:**

1. **Subscriber tags seed with the route's own `path:` tag** — the spec's
   `Sub.tags` comment names "route config tags + tags recorded by the last
   run", but the loader/cache surfaces carry no per-route tag declarations
   to read (`RouteLive` has no tags field; `LoaderOutput` has no recorded
   tags; `/__data` responses are never L0-cached). The fan-out covers both
   `tag` and `path` revalidate events, and `Hub::retag` stays for the
   future load-recorded tags — there is no invented API reading tags that
   do not exist.
2. **The `AST-LIV-001` refusal** (`live: true` on Static) belongs to the
   refusal-rule WP (spec §2776 `check_config`); the matrix row it consults
   is in place. The `tests/live.sh` integration script rides with it (the
   `#[ignore]` e2e pin `live_pushes_snapshots_over_the_wire` + its
   `live SSE e2e` CI step covers the same contract over the wire today).
3. **The `transport: 'poll'` fallback still polls full snapshots;**
   converting poll responses to patches is pointless while ETags gate the
   round trip.
4. **Capability row choices the spec sentence leaves open**: Deno/Bun =
   Full (NodeHost-backed runtimes with full §18 sockets rows); Cloudflare/
   CloudflarePages = Degraded (Workers stream but do not guarantee
   long-lived connections) — both documented in `degradation()`.

**Hygiene rides-along (recorded because the gates demand them):**

1. **Workspace resolution was broken at `v0.3.0`**: the A04 version-bump
   commit moved `[workspace.package] version` to 0.3.0 while the 18
   internal `[workspace.dependencies]` entries still required `0.1.0` —
   `cargo metadata`/`cargo fetch`/any `cargo build` failed on the tagged
   tree. Fixed by dropping the version reqs from the internal entries
   (path-only): the version stays single-sourced in `[workspace.package]`
   and ship.sh's next bump cannot re-break resolution.
2. **The root `package.json` was invalid JSON**: `"version": "0.3.0",,`
   — a doubled comma the previous ship's version-bump edit spliced in,
   present on the tagged `v0.3.0` tree (`json.load` rejects the file).
   Fixed; the F05 ship's version-bump step rewrites the whole
   `"version": "…"` line instead of appending to its value.
3. **`--all-features` flipped the schema byte-gates red** (pre-existing,
   proven on the pin-fixed tree): `bundler`'s feature graph pulls
   `serde_json/preserve_order`, which reorders every `serde_json::Value`
   map in the unified build — `config_schema_pretty()` emitted unsorted
   and both `schema_stability.rs` and `schema_tdd.rs` failed. Fixed by
   canonicalizing the emitter through a BTreeMap projection (sorted keys
   under any feature unification; a no-op with the default flavor); the
   checked-in artifact regenerated (byte-identical under default flags).
3. **`server`/`render`/`adapters` have not adopted `[lints]
   workspace=true`** — their combined pre-existing pedantic debt
   (~70 sites across the 6.2k-line server pipeline, render, adapters)
   belongs to the planned dedicated hygiene pass (the workspace Cargo.toml
   note: "the remaining V8-free crates adopt pedantic in a dedicated
   hygiene pass"). Every site the F05 substrate added is pedantic-clean
   by direct check; `server` gained a dev-only `tokio` `test-util`
   feature (paused-time pins) and the `bench-util` criterion harness
   (V2-B09 pattern).

## V2-F06 ODF patch deltas — deltas and deferrals (substrate step)

**Status.** Substrate landed (`crates/odf/src/patch.rs` — `Seg`/`Op`, `diff`,
`apply`, the `ODP1` envelope (`ops_to_value`/`value_to_ops`,
`encode_patch`/`decode_patch`, `base_id`), `crates/odf/tests/fixtures/patch/`
(12 committed cross-language goldens), `crates/odf/benches/patch.rs`,
`crates/server/src/live.rs::delta_payload` + `Sub::prepare_rerun` (the D30
sending policy on the F05 runner path), `packages/core/src/client/
odf-decode.ts` (`decodeOdfOrPatch`/`applyPatch`/`sha256Hex16`) and
`packages/core/src/client/live.ts` (the default decode swap +
reconnect-on-refusal)). The endpoint/rerun-closure wiring stays with the
A01-phase-3 work (V8-capable host) — the F06 patch transport ships inside
the existing F05 substrate and activates there with zero further changes.

**Deliberate deltas from the spec text:**

1. **`apply`/`decode_patch` return `Result<_, OxideError>` with `ODP-*`
   codes** (`ODP-MAGIC`, `ODP-SHAPE`, `ODP-OP`, `ODP-PATH`, `ODP-DEL`,
   `ODP-SPLICE`, `ODP-KEY`) instead of the spec's `Result<Value, String>` —
   the crate's error type is the diagnostics `OxideError`, and callers
   (server policy, client reconnect) branch on codes, not strings.
2. **Reserved keys are rejected at decode time, not apply time.** The spec
   pins the TS `applyPatch` rejection of `__proto__`/`constructor` paths;
   the Rust side rejects them in `path_from_wire` (`ODP-KEY`) — a patch that
   writes a reserved key is malformed at the parse boundary, and the TS side
   matches (`applyPatch` checks every segment before walking).
3. **The client hashes with a compact synchronous SHA-256**, not the spec's
   "sha-256 Web Crypto": the live decode hook is synchronous
   (`LiveDecode = (payload, last) => unknown`), and `crypto.subtle.digest`
   is async. The implementation is pinned by the RFC vectors plus every
   committed golden (the `b` field inside each `ODP1` file was produced by
   Rust's `sha2`, so the fixture suite IS the cross-language hash parity
   test). The spec's "do not use blake3 on the client" constraint is kept.
4. **Wire integers run through the exact-integer range (≤ 2^53) and a
   cast-free parse** (`f64 → decimal string → u64 → usize`); encode-side
   indices convert through `u32` — unreachable in practice (a 256 MiB
   payload holds ≤ 2^28 elements) and encoded failures surface as
   `ODP-SHAPE` rather than silent truncation. No f64↔int casts anywhere
   (the workspace is cast-free by policy, and `#[allow]` is unavailable).
5. **`decode_patch` resolves container dedup refs before parsing ops.** The
   ODF encoder aliases bit-exact-equal containers into `Ref(i)` slots
   (e.g. an empty Splice `insert` aliases an empty `path`), so the patch
   decoder decodes with the shared table and resolves — without this the
   truncate/no-op goldens do not even parse.
6. **The patch base lives on the subscriber as the last FULL snapshot**
   (`Sub::last_full`); the `watch` channel carries whatever the policy
   produced (patch or full), and `push_snapshot` (the connect path) seeds
   the base so the first rerun can already patch. A fresh connection always
   receives a full snapshot — the F05 invariant — because a fresh `Sub`
   starts with no base.
7. **The client re-encodes the applied value with the canonical TS encoder
   to compute its next base id** (the server's next patch chains from the
   full it would have sent). For the value domain the goldens pin
   (`encodeOdf` ≡ Rust `encode`), ids advance in lockstep; any drift (a
   resolved-ref cycle, an exotic type) registers no base and the next patch
   refuses with `ODP-BASE` — reconnect, fresh snapshot, graceful
   degradation, never corruption.

**What remains deliberately deferred:**

- Nothing F06-specific: the transport is active end-to-end on the F05
  wiring (server policy ⇄ SSE frames ⇄ client decode). The remaining F05
  deferrals above (`AST-LIV-001`, `tests/live.sh`, the poll fallback)
  apply unchanged.

**Hygiene rides-along:** the `odf` crate-module doc pointed at a
nonexistent `docs/format.md` and a nonexistent `packages/oxide` package —
corrected to the real paths (`packages/core/src/client/odf-decode.ts`, the
fixtures directories), with a note recording the drift. The other drift
found this session (README crate count, CI job counts, spec Appendix V2.O's
all-`todo` table, CONTRIBUTING's toolchain claim) is listed in the handoff
and left to the doc owners.

**Unclosed:** the wiring landed (`feat/live-loaders-wire`, 2026-11-15).
The e2e that asserts the wire contract is the `#[ignore]` pin
`live_pushes_snapshots_over_the_wire` (`crates/cli/src/tests.rs`) run by
CI's `live SSE e2e` step: same-origin 403s, the full connect frame, and a
new data frame after the fixture action's `revalidate.path` — the F06
frame-magic sanity check rides it (the exact full-then-patch shape is
pinned V8-free in `frames_carry_full_then_patch_payloads`; the sending
policy may legitimately ship a full for tiny envelopes, which the e2e
honors). The `tests/live.sh` curl script remains with `AST-LIV-001`.

## V2-F10 skew protection — the two-pool serving deltas (wired)

**Status.** The id-level wiring (the A04 stage, the bounded
`GenerationPool`, the SIGHUP promote, the expiry task, the skew e2e) was
already landed. **The serving half landed** (`feat/live-loaders-wire`,
2026-11-15): a pool + loader runner per generation, routed by build id.

**Deliberate deltas from the spec text:**

1. **The loader bytes freeze via a source snapshot, not the pool.** The
   spec's step 1 ("keeps G1's **server module and pool** alive") assumed
   the pool alone preserves the old bytes — but the worker recompiles
   loader modules per run with an mtime check (`compileServerModule`'s
   B-13 fast path), so a demoted pool re-reading an EDITED source serves
   the NEW bytes. The wiring freezes each generation's loader sources
   under `.oxide/generations/<id>/src` (a deep copy at boot and at every
   promote — the bytes are frozen at the moment the generation becomes
   current) and the demoted generation's runner resolves its module paths
   from its own snapshot root; node-package imports resolve through the
   ancestor walk to the project's `node_modules`. A failed snapshot
   downgrades the generation to acceptance-only (a `SKW-SNAPSHOT` warn;
   the boot handles serve every generation) instead of failing the boot.
2. **The promote boots a full fresh stack** (`build_framework_stack`) —
   the new pool + runner — and `GenerationPools::promote` demotes the old
   handles together (`ServingPools::promote_booted`). The route step
   (`crates/cli/src/lib.rs`'s dispatch prelude) resolves the generation by
   `x-oxide-build` on the data plane and shadows the boot handles; HTML
   navigations always ride current (§V2-F10 step 2 verbatim). The
   acceptance verdict stays the one stage (the wrapper drives the id-level
   pool) — the routing layer only decides WHO serves an accepted id, and
   the dispatch-crate pins (`pools_tdd.rs`) hold the semantics.
3. **The rollback path's `--generation previous` flag and the
   `.oxide/generations/<id>` hard-link layout** (spec step 4) stay
   deferred — the snapshot directory now doubles as the frozen-source
   store; the flag rides the F11 releases slice.
4. **The brownout early-drop hook** (`evict_oldest`) is the substrate's
   (pinned); the F12 wiring calls it when the queue-wait threshold trips.

**Wire proof:** `tests/skew.sh` step 4 now asserts BOTH the acceptance
contract AND the old bytes — the old-header `/__data` still carries the
v1 loader output (the frozen snapshot) while the headerless request
carries v2, and the serve log names the booted generation's pool. CI's
`skew e2e` step runs it (V8-capable host).

## V2-F07 resumable streams — deltas and deferrals (substrate step)

The V2-F07 substrate (`server::stream_store`, the `/__oxide/stream/<id>`
data-plane seam, the `stream.resume` config surface, the client
`resumableFetch`) landed V8-free in the A02/A05/F04/F05/F06 pattern.

**Deliberate spec deviations:**

1. **The reattach wait is a `watch` channel over a `Progress { len, done }`
   value, not the spec's `Notify::notify_waiters`.** `Notify` waiters
   register only on their first poll, so a chunk appended between a
   reader's backlog check and that first poll is missed — on a multithread
   runtime the reader then sleeps forever and the reattach hangs
   mid-stream. The `watch` channel (the F05 `Sub` pattern) stores the
   latest value and resolves `changed()` for any send after the last seen
   one, so no state transition can be missed; the buffer holds a primary
   receiver, making every send infallible.
2. **Overflow removes the buffer from the store immediately** (chunks
   cleared, bytes paid back to the total accounting) instead of leaving a
   shell to be swept. The spec's `get` already answered `Gone` for
   overflowed buffers, so the observable behavior is identical; removing
   at overflow time keeps the total accounting exact and the memory bound
   hard.
3. **The total-accounting eviction protects the buffer being appended
   to.** "Evicts oldest completed first, then the oldest buffer" otherwise
   holds; an in-flight stream never loses its own replay mid-write (the
   per-stream `maxBytesEach` cap is the bound that stops a single runaway
   buffer). The bound is therefore `maxBytesTotal` + the one active
   stream's bytes, which the soak's memory bound still satisfies with one
   in-flight response per connection.
4. **`pump` constructs the body reader inside its `try`.** The spec's
   client pseudocode does `r.body!.getReader()` outside the try — a
   reattach answered with a body-less `503` escapes as a `TypeError`
   instead of counting as a failed try. (Caught by the non-ok-status pin.)

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

- The tee: wrapping resumable endpoint/server-fn response bodies with
  `tee_stream` in the dispatch closure, minting `stream_id` per response
  (the wiring owns the per-request counter), and adding
  `x-oxide-stream: <id>` to the response head.
- The route handler in the CLI server: `stream_resume_id` +
  `parse_range_offset` + `StreamStore::get` + `stream_resume_response` are
  the whole handler; the CLI closure assembles them on
  `/__oxide/stream/*`.
- The per-IP reattach rate limit (the spec's step-3 limit) rides the
  existing rate-limit table the pipeline already owns (`server`'s
  sharded buckets, PERF-22).
- The housekeeping task calling `StreamStore::sweep` on a 30 s cadence,
  and the `stream.resume` knobs flowing from config into
  `StreamStore::new(ttl, max_each, max_total)`.

**Hygiene rides-along:** `crates/dispatch/src/build.rs`'s A04 note
("`/__oxide/stream/*` joins when that route lands (F07) — until then it
must not enforce the header") is closed: the route joined the data plane
and the A04 pins that documented the pre-F07 world were updated in place
(the F07 pins live in `crates/dispatch/tests/stream_tdd.rs`).

**To unclose:** the A01 phase-3 wiring above, then the e2e: an endpoint
emitting numbered lines, `curl --max-time` dropping the first connection,
a reattach with the byte offset, and the concatenation equal to the full
stream byte for byte.

## V2-F12 brownout mode — deltas and deferrals (substrate step)

The V2-F12 substrate (`server::governor`, the `Priority`/`serve_decision`
enforcement seam, the `oxide_brownout_level` gauge, the `brownout` config
surface, `NativeRule.priority`) landed V8-free in the
A02/A05/F04/F05/F06/F07 pattern: the pure model and its pins, the seams
the wiring plugs into, the config + schema surface, the criterion bench.

**Deliberate spec deltas:**

1. **The `rss_mb` config rungs use `0` as the documented "no RSS gate"
   spelling** (JSON cannot carry the spec's `u64::MAX` sentinel without
   precision loss); the wiring maps `0` → `u64::MAX` when it builds
   `Thresholds` from the config section.
2. **The threshold ladders are milliseconds in config, microseconds in
   code.** The spec's `Thresholds::default` and `Signals` are
   microseconds (`queue_wait_p99_us`, `loop_lag_us`); the config section
   carries whole milliseconds (`queueWaitMs: [50, 200, 500]`,
   `loopLagMs: [20, 100, 300]`) because §14 config knobs are human units
   (the F07 ttl is seconds, the size bounds bytes). The conversion is the
   wiring's, and `next_level`/`raw_level` stay spec-verbatim.
3. **Ladders are validated ascending at load (`DSP-113`)** — the spec's
   code does not check, but a rung below the previous one would
   de-escalate into a breach and oscillate; refusing it at load is the
   config crate's established contract (DSP-110/111/112 precedent).
   `hedge.afterFactor` must be > 1 and `staleCeiling` must parse to a
   multiplier ≥ 1 under the same diagnostic.
4. **The hysteresis ratio is an integer percent (`exit_ratio_pct: u64`,
   default 60), not the spec's `exit_ratio: f64` 0.6.** The zero-allow
   lints policy forbids both `#[allow]` escapes and the `u64`→`f64` casts
   the float version would need (`cast_precision_loss`); the percent form
   compares in lossless `u128` (`signal × 100 < rung × pct`), keeps
   `next_level` allocation-free, and makes `Thresholds` `Eq`. The spec's
   default 0.6 is exactly 60%, and the hysteresis-band pins cover the
   same boundary.
5. **`serve_decision` treats `Stale`-level service as `ServeStale` for
   every priority** — the header attaches when the caller actually serves
   past the SWR window, so `ServeStale` is "eligible", not "forced"; the
   cache wiring decides per lookup (the spec's `Stale` step is a
   permission granted to the L0 lookup, not a mandate).
6. **`Governor::set` clamps to 3.** The spec's `set` stores the raw byte;
   a sampler bug writing 4 would map to `Emergency` on read anyway, and
   clamping keeps the `x-oxide-brownout` payload and the gauge in the
   documented 0–3 range.

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

- The sampler task: a 500 ms tick reading `pool.stats()`
  (`oxide_pool_queue_wait_p99_us`), the loop-lag probe (sleep 100 ms,
  record the overshoot) and RSS (`/proc/self/statm`), running
  `next_level` and publishing through `Governor::set` +
  `Metrics::set_brownout_level`. All of it is cli-side wiring around the
  landed model.
- The enforcement stage after prelude: `serve_decision` +
  `shed_response` per request, the `warning: 110` header on stale
  L0 service, and the `brownout.staleCeiling` multiplier flowing into the
  L0 lookup's SWR window.
- The readiness flip (`ready_status` → `/_oxide/ready`) and the
  `/_oxide/health` level report — both are cli-side response assembly.
- The hedger (opt-in, off by default): duplicating a still-running
  idempotent GET render after `afterFactor` × p95 and racing the results.
- The load test (`benchmarks/overload.sh` with `oha -c 500`) that proves
  the critical route's p99 stays flat while the slow route sheds.
- The brownout shrink of F10's previous-generation pool (the F10 expiry
  consults `oxide_pool_queue_wait_p99_us` against the brownout threshold).

**Grammar note:** the `priority` grammar (`low | normal | critical`) is
validated in the config crate (a literal match in `validate`) and parsed
again by `governor::parse_priority` in the server crate — the NativeRule
mirror already duplicates the struct across the dep boundary (config
cannot depend on server), so the grammar is pinned by tests on both sides
(`brownout_config.rs` load pins, `governor_tdd.rs` parser pins).

**To unclose:** the A01 phase-3 wiring above, then the e2e: `oha -c 500`
against a slow dynamic route with a `critical` L0 route alongside — the
critical route's p99 stays within 2× its idle p99 while the slow route
sheds with `503` + `x-oxide-brownout: 2`, the exposition shows
`oxide_brownout_level 2`, and `/_oxide/ready` stays 200 until Emergency.

## V2-F03 interaction replay — deltas and deferrals (substrate step)

**What lands.** The full client substrate in `packages/core`:
`src/client/replay.ts` (capture in the DOM capture phase against pending
`<ox-island>` hosts, the 100-record drop-oldest log, host-relative child
paths, replay-on-`done` with node-identity verification), the
`hydrateIsland` wiring (replays the island's records immediately after
`data-ox-hydrated` becomes `done`), and the boot wiring (`boot()` and
`bootWithRouter()` both start the capture before any island hydrates; the
payload's `interactionReplay: false` opts out). 15 Node pins in
`packages/core/test/replay.test.ts` + the `replay.bench.ts` micro-bench.

**Spec pseudocode deltas (4):**

1. **Records carry the captured `target`; replay drops when the resolved
   node is not the captured one.** The spec's `!el || !host.contains(el)`
   guard cannot detect a same-slot replacement: after a defer swap the
   recorded path resolves to the NEW node occupying the index, which IS
   contained in the host — the spec sketch would replay the click onto the
   wrong element. Node identity (the element the event actually hit)
   survives in-place hydration and fails across a content swap, which is
   the invariant the spec states ("element replaced by hydration or a
   defer swap: drop").
2. **`pathOf` refuses paths that do not terminate at the island host**
   (returns `null`; capture skips the record). The spec sketch walks
   `parentElement` unguarded and records a truncated path for detached /
   template-fallback targets; the observable contract is identical (the
   record can never replay) without materializing garbage.
3. **Replay restores `checked` for `change` records.** The spec sketch
   records `checked` but never applies it, so a pre-hydration checkbox
   toggle would lose the user's input on replay.
4. **`startReplay` is idempotent per document** — a second call for the
   same document returns the existing stop handle instead of installing
   duplicate capture listeners. `boot()` can run more than once across
   duplicate module evaluations (bundler dedup failure, SPA reimport);
   the spec sketch would double-record every interaction and replay each
   one twice. The module-global log also carries `resetReplay()` /
   `replayPending()` hooks for the Node test runner and devtools;
   production boot never calls them.

**What is deliberately deferred (Rust config surface — this session's
sandbox has no Rust toolchain):**

- The `interactionReplay` config section end-to-end:
  `oxide.config.ts` → typed section in `crates/config` (+ load validation
  under a DSP number) → regenerated `schemas/oxide-fw-config.schema.json`
  → the hydration bootstrap emitting the field into `window.__oxide`.
  The client side of the seam is already live and spec-shaped: the boot
  runtime honors `payload.interactionReplay === false` today, so emitting
  the field is the only remaining change and defaults stay ON (flag
  absent = enabled).
- The Playwright acceptance (block the island chunk request, click the
  SSR button, release, assert the handler count is exactly 1) — needs a
  real browser plus a built smoke app. The Node pins cover the same
  contract with fakes; `tests/playwright` gains the block-release spec
  when the browser harness is available.

## V2-F16 why-dynamic — deltas and deferrals (substrate step)

**What the spec asks.** `infer_reasons(source, path) -> Vec<DynamicReason>`
beside `infer_render_mode`, the `.oxide/routes.reasons.json` side-car,
`oxide why <route>` printing each reason with a code frame, and `--fix`
codemods verified by re-running the same engine.

**What lands.** The full Rust substrate: `ReasonKind` (the four §10.1 kinds
plus `UncachedFetch`), the collect-all visitor that reuses the same walk the
mode inference uses, the `OX-DYN` diagnostics with `render_code_frame`
frames and the per-kind fix help text, `oxide why --route <id>` (mutually
exclusive with the module argument, id accepted in scanner, canonical and
bare directory form), and the side-car written by `oxide routes` and
`oxide build` — keyed by the canonical directory-form route id.

**Spec deltas:**

1. **A destructured bind and its member use are two reasons.** The spec's
   example treats "why is this route dynamic" as one answer per read; the
   walk reports the params bind (`({ cookies })`, the span `oxide routes`
   has always pointed at) *and* each member use, in pre-order. The first
   non-`UncachedFetch` reason is exactly the mode trigger, so the report
   and the route table never disagree.
2. **An `UncachedFetch` never flips the render mode.** The spec lists
   `UncachedFetch` as a reason kind but does not say whether it affects
   `InferredMode`; the mode contract is frozen (§10.1 reads request-scoped
   *state*, a fetch reads none), so `fetch.server.ts` reports `static` with
   one reason. `infer_render_mode` scans past fetch hits to preserve its
   first-hit behavior bit-for-bit.
3. **Reasons are only collected where the mode is actually inferred.** A
   route with an explicit `config.render` skips the AST inference and the
   side-car row — the why report accompanies the *inferred* decision, and
   the scanner's `render_mode` value is the authority there.
4. **The side-car write failure is a `tracing::warn!`, not a build error.**
   The file is advisory (never on the request hot path, never in the rkyv
   archive); failing a deploy over a report file would invert the priority.

**What is deliberately deferred (`--fix` codemods):**

- The four codemods (cookie/header read → loader data, `searchParams` →
  declared `searchSchema`, uncached fetch → `'use cache'`) run in the
  tooling pool as TS transforms (§6.2) and need the Node tooling host; the
  Rust substrate ships the verification surface they will reuse
  (`infer_reasons` re-run + mode re-check, the "refuse if the mode did not
  improve" guard). The `OX-DYN` fix help text already states each codemod's
  contract, so the registry row and the deferred flag agree.
- The inspector route-table `why` column (E08) and the MCP `routes.list`
  reasons field read the side-car; both are deferred with their surfaces.

## V2-E01 HMR boundary planner — deltas and deferrals (substrate step)

**What the spec asks.** `crates/dev/src/boundary.rs`: given the module
graph (`importers`, the self-accepting set, the entry set), find the update
boundaries for a changed file by walking importers breadth-first until a
self-accepting module is reached on every path; if any path reaches an
entry module without a boundary, return `FullReload`; cycle-safe
(visited set). Message schema, state preservation, CSS boundary algorithm
and the Vue/Solid refresh wiring are the rest of the WP.

**What lands.** The full Rust substrate: `boundary::Plan`
(`Update(Vec<PathBuf>)` / `FullReload(String)`, spec-verbatim shapes) and
`boundary::plan` (the spec's BFS, boundaries sorted + deduplicated), the
`source_self_accepts` probe (the `import.meta.hot.accept` registration
test), the `DEV-HMR-001` full-reload diagnostic with the per-cause fix
help text, `hmr_boundary` tracing spans on the planner, the criterion
bench behind `bench-util` (deep chains, wide fan-outs, diamond dedup, the
reload fallback), the walkthrough example, and the CLI wiring: the
adapter-HMR fan-out in `cmd_dev` is planner-driven — `build_hmr_graph`
harvests `importers`, `accepting` and `entries` in the same walk the old
`build_importers_map` already paid for.

**Spec deltas:**

1. **The changed file always re-imports.** The pure planner returns only
   the boundary set (when the changed module itself is not a boundary, it
   is not in `Plan::Update`); the dev loop keeps the changed file at the
   head of the batch, because the boundary's re-import must observe the
   new bytes of the module that triggered it and the `?v=<hash>` bust only
   flows through a re-request the client issues for the changed URL.
2. **Orphans pre-filter to a quiet self-update.** The spec's `_ =>` arm
   returns `FullReload("<module> is not imported")` for a module nothing
   imports. That is right for the pure function (and the pins exercise
   it), but the dev loop cannot reach any client with such an edit — a
   reload would be user-visible noise for a no-op (the §1.3 boot-storm
   lesson). `cmd_dev` checks the changed file's importer row first and
   only consults the planner when something imports it. A dead-end chain
   *below* the changed file still reloads (spec-verbatim: one boundary-less
   path poisons the plan) — over-notify is the safe direction, and the
   `DEV-HMR-001` reason names the dead-end module.
3. **Component modules are boundaries by framework-plugin convention, not
   by `import.meta.hot.accept` scan alone.** The spec names both sources
   of the accepting set; the harvest marks component extensions
   (`.tsx`/`.jsx`/`.vue`/`.svelte`), `.server.ts(x)` swap files and
   hot-accept registrations. CSS never enters the graph (the walk covers
   JS/TS/Vue/Svelte extensions only), so the CSS boundary algorithm
   remains the deferred half of the WP.
4. **`update_boundary` is retired, not wrapped.** The one-hop
   simplification (single-importer walk, multi-importer bail-out) was dead
   code outside its own tests and its doc claimed the real walk-up "lives
   in the graph engine" — it never did. The planner is the walk-up; the
   §13.3 pins moved to `tests/boundary_tdd.rs`. Also retired this pass:
   the `Watcher::with_notify` fallback reported through a bare
   `eprintln!`, bypassing every tracing subscriber — it is a
   `tracing::warn!` now (`oxide-fw-dev` gains the workspace-pinned
   `tracing` dependency for it).

**What is deliberately deferred (the rest of E01):**

- State preservation across boundary swaps and the CSS boundary algorithm
  (`plan` over style-graph edges) — both consume `Plan` unchanged.
- Vue and Solid refresh wiring: the planner's fan-out is
  framework-agnostic; the per-framework in-place patch is the adapter
  packages' track (React Refresh already owns its updates through the
  `react_refresh_owns_update` skip).
- The Playwright transitive-HMR pins against a real browser: the existing
  `hmr-transitive` spec pins today's contract, which the planner is a
  strict superset of (deeper chains now update instead of stalling).

## V2-F20 attested reproducible builds — deltas and deferrals (substrate step)

**What the spec asks.** `oxide build --attest` writes `dist/attest/`:
`sbom.cdx.json` (CycloneDX 1.5 from `Cargo.lock` for the binary plus
`pnpm-lock.yaml` for packages that reached a bundle, own serde structs, no
external generator), `provenance.intoto.json` (in-toto Statement v1 with
the `https://slsa.dev/provenance/v1` predicate; subjects are the SHA-256
of `dist/build-id`, the binary and each top-level artifact;
`buildDefinition` carries the oxide version, config hash, git commit and
dirty flag; `runDetails.builder.id` from `OXIDE_BUILDER_ID` or `local`),
and `provenance.sig` (ed25519 over the canonical JSON, key from
`OXIDE_SIGNING_KEY` or `--key <file>`; the public key id is recorded).
`oxide verify <dist> [--pubkey <file>] [--rebuild]` checks the signature,
the subject digests and, with `--rebuild`, the rebuild's `dist-digest`.
Determinism requirements (tested): no timestamps in artifacts
(`SOURCE_DATE_EPOCH` for any needed time), sorted maps and file lists,
path normalization (strip the project root), identical `dist-digest` for
two builds in different directories.

**What lands.** The full pure substrate in `crates/cli/src/attest.rs` —
`canonical_json` (explicit recursive key sort, so the bytes stay sorted
even if a dependency unifies in `serde_json/preserve_order`), `is_canonical`
(the verify gate that makes a whitespace-only tamper fail before the
signature check), the strict `Cargo.lock` / `pnpm-lock.yaml` line parsers
(own code, no external generator or new dependency), `sbom_from_locks`
(CycloneDX 1.5, `pkg:cargo`/`pkg:npm` purls, sorted components),
`provenance_statement` (the spec's exact `_type` / `predicateType` /
`buildDefinition.externalParameters` shapes, `internalParameters.
sourceDateEpoch` only when the epoch is set), `subtree_digest` +
`dist_subjects` (root-relative, slash-normalized, sorted, `attest/`
excluded so the statement can never self-reference), `seed_to_pkcs8` +
`load_signing_key(_bytes)` + `ed25519_pubkey_from_pem` (ring's ed25519 —
already in the workspace graph — so no new dependency),
`sign_statement`/`verify_signature`, `verify_subjects`,
`write_attestation`, and the `ATT-*` diagnostic taxonomy with fix lines.
Command surface: `oxide build --attest [--key <file>]` (the `cmd_build`
tail) and `oxide verify <dist> [--pubkey] [--rebuild]`. The criterion
bench behind `bench-util`, the walkthrough example
(`crates/cli/examples/attest.rs`), and 24 TDD pins in
`crates/cli/tests/attest_tdd.rs` including the two-builds-different-
directories reproducibility pin and the one-byte-tamper pin.

**Spec deltas:**

1. **The signature envelope is a small JSON file, not bare bytes.**
   `provenance.sig` is `{ "keyid": "<hex public key>", "sig": "<base64>" }`
   — the spec says "the public key id is recorded", and a self-describing
   envelope lets `oxide verify` name the missing key (`signature: skipped
   (pass --pubkey <file> to check keyid ...)`) when no verifier key is
   supplied. Without `--pubkey`, the signature check is skipped loudly and
   the digests still verify; the envelope records only the key id, so a
   keyless verify can never be mistaken for a signed one.
2. **Verification is a two-gate flow: canonical, then signature.** The
   spec's "tampering with one byte fails `verify`" is enforced before the
   crypto runs: `oxide verify` first requires the file bytes to equal
   `canonical_json(parse(bytes))` (`ATT-CANON`), so a re-serialized or
   whitespace-tampered statement dies even though a signature over the
   canonical form would still match the parsed value.
3. **Subjects: files hash whole; directories hash as subtree manifests.**
   A top-level directory (`dist/client/`, `dist/server/`) contributes one
   subject whose digest is the SHA-256 over the sorted
   `path<TAB>sha256` manifest of its files — the same shape
   `cache::build_id_of_dist` uses for the A04 id (SHA-256 where that uses
   blake3, because the subject digests are the cross-implementation
   surface the spec fixes). `dist/attest/` is never a subject, so the
   statement cannot self-reference its own output.
4. **A project without a `Cargo.lock` attests with an empty cargo
   section.** The spec's "SBOM contains known crates" acceptance runs
   inside this workspace, where the lock exists; a plain user app (no
   cargo workspace) gets the npm components only, and the report says so.
   The release-binaries CI always builds in-workspace, so the published
   artifacts always carry the crate list.
5. **The pnpm side lists every locked package — the "reached a bundle"
   filter is deferred.** Intersecting `pnpm-lock.yaml` with the client
   manifest's source list needs the bundler's module-to-package map on
   the attest path; until that lands, the SBOM over-reports (every locked
   package, not only the ones that reached the bundle). Over-reporting is
   the safe direction for an SBOM.
6. **`--rebuild` compares every subject except `dist/build-id`.** The
   scratch build runs without `--attest` (verify holds no signing key),
   so the rebuilt `dist` has no `build-id` file; the id is derived from
   the same content the other subjects pin, so matching
   `dist/client/` + `dist/server/` + every top-level file implies a
   matching id by construction.

**What is deliberately deferred:** the bundler-side "packages that
reached a bundle" manifest filter (delta 5); the A01-phase-3 unconditional
`dist/build-id` write (today it is written under `--attest` only, which is
the only consumer the substrate has); a DSSE envelope upgrade for
`provenance.sig` (the spec asks for a bare signature, so the envelope is
the shipped shape); and the CI acceptance job that builds twice in
different paths and runs `verify --rebuild` (the pure reproducibility
contract is pinned by `write_attestation_is_reproducible_across_directories`;
the end-to-end job needs the release CI wiring).

## V2-E01 phase 2 — CSS boundary substrate — deltas and deferrals (substrate step)

**What the spec asks.** The message schema (version 1) with the
`kind: 'js' | 'css' | 'data'` discriminant, and the CSS boundary algorithm:
"`kind: 'css'` swaps the `<link>` or `<style data-ox-css=path>` without
reload" — CSS joins the planner instead of living in a dev-loop
side-channel (the phase-1 record: "CSS never enters the graph").

**What lands.** The pure substrate in `crates/dev/src/boundary.rs`:
`UpdateKind` (the wire discriminant; `as_str` is the spec-verbatim
spelling), `update_kind` (the SPEC-14 `ChangeKind` → wire-kind bridge),
`css_self_accepts` (the extension-level style-injection probe),
`plan_classified` + `ClassifiedPlan` (the fan-out's decision function —
`ChangeKind` + the `plan` walk in one call, with the §13.3 CSS terminal: a
`FullReload` walk for a stylesheet collapses to `Update([changed])`, the
swap), and `css_orphan_diagnostic` (`DEV-HMR-002` with the fix help text).
`hmr_boundary_classified` tracing spans on the seam. Ten new pins
(`tests/boundary_tdd.rs` phase-2 section + the `try_to_json` unit pin),
three criterion groups behind `bench-util` (`boundary_css_terminal`,
`boundary_css_edge`, `boundary_classified_js`), and the `css_boundary`
walkthrough example. The CLI's `build_hmr_graph` walks
`css`/`scss`/`sass` too and marks stylesheets self-accepting; the
`cmd_dev` StyleSwap branch (ESM, in-`src/`) routes its decision through
`plan_classified` — the swap frames are byte-identical to the
pre-substrate wire, and the only new traffic is the `DEV-HMR-002` warning
frame for an orphaned in-`src/` stylesheet (the swap still runs — a static
`<link>` may carry the sheet; the warning is additive and the overlay
renders it without a reload).

**Spec deltas:**

1. **The terminal lives in the planner, not the transport.** `plan` itself
   stays spec-verbatim (an orphan stylesheet walk returns
   `FullReload("... is not imported")`); the collapse happens in
   `plan_classified`, so the pure function's contract is unchanged and the
   pins exercise both shapes.
2. **Stylesheets only receive edges, never emit them.** The harvest adds
   `css`/`scss`/`sass` to the walk (JS→CSS side-effect imports resolve
   exactly — their specifiers always carry the extension), but a CSS
   `@import` chain is NOT scanned into the graph: the changed child sheet
   self-accepts and swaps on its own, so parent edges would add walk cost
   with no behavioral difference.
3. **The orphan-CSS case warns instead of reloading.** The spec's `_ =>`
   arm is right for the pure function; the dev loop collapses it (the swap
   still runs) and publishes `DEV-HMR-002` so a stylesheet the module
   graph cannot see is visible in the overlay and the logs.

**What is deliberately deferred (the rest of E01):**

- The message-schema v1 wire flip (`{ v: 1, type: 'update', updates:
  [...] }` with `kind`/`acceptedPath`/`timestamp`/`hash` per item): the
  Rust enum's kebab-case tags (`module-update`/`css-update`/...) stay the
  wire truth until the TS client, the four adapter test suites and the
  Playwright pins move together — `UpdateKind::as_str` is the substrate
  the flip consumes.
- State preservation across boundary swaps and the Vue/Solid refresh
  wiring (adapter-track work; the planner fan-out is framework-agnostic).
- The Playwright CSS-state acceptance pins (edit a CSS file, no navigation
  event fires) against a real browser.


## V2-E01 phase 3 — message-schema v1 wire flip — deltas and deferrals (substrate step)

**What the spec asks.** The message schema (version 1), in
`packages/oxide/src/client/hmr.ts` and `crates/dev`:
`{ v: 1, type: 'update', updates: [{ kind: 'js'|'css'|'data', path,
acceptedPath, timestamp, hash }] } | { type: 'full-reload', reason } |
{ type: 'error', diagnostic } | { type: 'ox:data', route, data } |
{ type: 'connected', build }`.

**What lands.** `HmrMessage` restructured to the v1 variants behind a
`{"v":1}` envelope (`to_json`/`try_to_json` stamp it; `try_from_json`
enforces it with the typed `HmrWireError::Schema`), `UpdateEntry` +
`UpdateEntry::stamped` (the fan-out's entry constructor: `unix_millis()`
timestamp, `content_hash` blake3 prefix), `schema_mismatch_diagnostic`
(`DEV-HMR-003` with the fix help text), the retired `from_json` `.ok()`
swallow now warning through `tracing::warn!`, the CLI publish sites
migrated to stamped entries (the §3.2 batch tail emits one `Update` frame
with N entries), the TS client's one-code-path switch with the legacy
read-compat normalizer (`normalizeLegacyFrame`), the worker socket in
`hot.ts` riding `acceptedPath`, the criterion bench (`hmr_wire`), the
`hmr_wire` walkthrough example, and 10 new client pins plus the rewritten
Rust wire pins (1,750→1,756).

**Spec deltas:**

1. **`kind` is the client's update mechanism, not the changed file's
   extension.** A `src/` stylesheet swaps through its `/@css/` module, so
   its entry is `kind: 'css'` with `acceptedPath: '/@css/...'` — the
   client re-imports the module (style injection, never a reload), which
   is exactly the §13.3 "swaps the `<link>` or `<style data-ox-css=path>`
   without reload" semantics; a stylesheet with no module owner (a static
   `<link>`) carries `acceptedPath == path` and swaps the href.
2. **`ox:data.data` rides as base64.** The JSON transport needs text; the
   pushed ODF payload is base64 (the v0 field was `odf_b64`). A data
   change WITHOUT a payload is not published as `ox:data` at all — it is
   an `Update` entry with `kind: 'data'` (the client re-fetches
   `/__data`), keeping the schema's two data flavors distinct.
3. **The tailwind sheet's inline `code` payload is retired.** The v0
   `CssUpdate{path, code}` carried the rebuilt CSS but no client read it
   (`swapCss` swaps the href; the browser refetches). The v1 entry is not
   file-backed (`hash: ""`) and the swap refetches through the
   cache-busted URL.
4. **`server-module-swap` is retired from the wire.** The frame was a
   client no-op (the DOM stays put; the watcher's
   `invalidate_framework_modules` bump owns the server-side swap), so the
   server stops publishing it and the legacy normalizer maps it to an
   empty `update` (the same no-op).
5. **The v1 gate is strict on the Rust side, compat on the client side.**
   `try_from_json` rejects v-less and foreign-version frames with
   `HmrWireError::Schema` (the server never decodes client frames; the
   gate pins the flip honestly); the TS client keeps a read-compat
   normalizer because a stale tab across a CLI restart holds the previous
   session's runtime against the current server's frames.
6. **`UpdateKind` gains its serde spelling.** The phase-2 discriminant
   enum now derives `Serialize`/`Deserialize` (lowercase) — `as_str`
   remains the TS-switch contract; the derives make the enum the wire
   type inside `UpdateEntry` directly.

**What is deliberately deferred (the rest of E01):**

- State preservation across boundary swaps (consumes `Plan`/
  `ClassifiedPlan` unchanged) and the Vue/Solid refresh wiring — the
  adapter-track work.
- Retiring the client's legacy read-compat window once no stale tab can
  plausibly exist (a future minor).
- The Playwright HMR pins against a real browser with the v1 frames (the
  spec spellings in `tests/playwright/` are updated; the runs still need
  a toolchain + browser host).

## V2-A07 prerender error-status sidecar — deltas and deferrals (substrate step)

**What the spec asks.** In `write_prerender`, when the render status is not
200, write `<route>/index.html.status` containing the decimal status. At
serve start, scan `dist` once for `*.status` files into
`StatusSidecars(HashMap<PathBuf, u16>)` (no per-request I/O) and have the
static-asset responder apply the status.

**What lands.** The full substrate across both crates the WP names:
`server::static_status` — `StatusSidecars` (`scan`, the spec's stack walk,
malformed content ignored, a missing root an empty map; `status_for`;
`len`/`is_empty` for observability), `is_status_sidecar` (the exact
`.status` extension probe), `parse_status` (trimmed `u16`), and
`write_status_sidecar` — the write half of the contract, next to the read
half so the spelling cannot drift: non-200 writes the decimal sidecar, a
200 write REMOVES a stale sidecar. `StaticAssets` owns a
`status_sidecars` map filled by the same `scan_filtered` call the CLI
already made at serve start; `serve` applies the sidecar status to every
200 response (identity, precompressed variant and HEAD) and hard-denies
serving `*.status` files (the SEC-STATIC shape; the asset walk skips them
too). The CLI calls `write_status_sidecar` at all four `write_prerender`
emit paths (the two loader short-circuits, the two error bodies, and the
final injected page) and both incremental-reuse branches — a status that
flips to 200 across builds cannot keep serving the previous build's error
status.

**Spec deltas:**

1. **The write half lives in `oxide-fw-server`, not the CLI.** The spec
   puts the write inside `write_prerender` implicitly; landing it as a
   server-crate function keeps the `.status` spelling, the trim rules and
   the clear-on-200 contract in one place, and lets the two reuse branches
   (which bypass `write_prerender`) share it.
2. **The scan is wired through `StaticAssets::scan_filtered`, not a second
   startup pass.** The spec's `StatusSidecars::scan` stays spec-verbatim
   (its own walk); `scan_filtered` calls it once after the asset walk so
   the CLI's existing `load_client_assets` site needs no change.
3. **Sidecars are build-internal files.** The asset walk skips `*.status`
   and `serve` hard-denies the path (404, not 403 — do not confirm
   existence), so the status files can never download as octet-stream.
4. **Only 200 responses take the sidecar status.** The 304/206/416
   verdicts are conditional-request/range semantics and stay untouched;
   the precompressed-variant and HEAD paths carry the sidecar status so a
   HEAD probe and a compressed GET agree with the identity GET.
5. **A missing root scans to an empty map** (the spec's `read_dir` guard
   is honored literally): a half-built dist must not refuse to boot.

**What is deliberately deferred:**

- The Playwright acceptance run (build a smoke app with a prerenderable
  `routes/boom` page and `curl -sI` it) — the in-sandbox evidence is the
  9-pin TDD suite plus the `status_sidecars` walkthrough example, which
  exercises the same scan→serve→500 contract against a tempfile dist; the
  browser-run proof still belongs on a toolchain host.
- `serve_og_endpoint`/`serve_image_endpoint` and the dispatch (L0/SSR)
  paths are untouched: the L0 seed already carries the status for seeded
  routes, so the sidecar covers only the disk-served paths it must.

## V2-E01 phase 4 — state-preservation substrate — deltas and deferrals (substrate step)

**What lands.** The server half in `crates/dev/src/state.rs` (new module):
`Preservation` — the `preserve` verdict the §13.3 v1 `update` frame now
carries (`{"policy":"keep"}` or `{"policy":"clear","reason":...}`,
internally tagged; `#[serde(default)]` so a pre-substrate frame decodes as
`Keep`, the verdict those servers always meant — additive, not a schema
break); the injectable `StatePolicy` seam (`DefaultStatePolicy`: a
boundary `Update` keeps state, a `Plan::FullReload` clears it with the
planner's reason verbatim); and `update_frame(&ClassifiedPlan, &dyn
StatePolicy, updates)` — the single frame-construction seam under an
`hmr_state_preservation` tracing span. The CLI fan-out publishes every
`update` frame through it (the tailwind rebuild, the two /@css/ style
swaps, the non-ESM style swap, the data-entry fallback, and the §3.2
batch), with a tick-scoped `tick_reload_reason` tracker: when any event in
the same watcher batch planned a full reload, the accumulated batch frame
is stamped `clear` with the SAME reason the `full-reload` frame carries,
so the two frames tell one story. The client half in
`packages/core/src/client/hmr.ts`: `snapshotFormState`/`restoreFormState`
(form fields keyed name → id → ordinal path, plus the focused field;
restore is assignment-idempotent and gated on the async applies settling
via `Promise.allSettled`), `stashFormStateToSession`/
`restoreStashedFormState` (the fallback-reload stash under
`ox-hmr-state`, 10 s TTL, cleared on restore), and
`scheduleStashRestore` wired into `bootWithRouter`'s ESM-dev path. The
spec's third HMR acceptance — *type into an input, edit the component's
text, the input value survives* — now holds for boundary-scoped updates
on every adapter, including the fallback-reload path.

**Spec deltas:**

1. **The verdict rides the existing v1 frame instead of a new message.**
   The spec's schema names no preservation field; an additive `preserve`
   field on `update` keeps the schema version at 1 (no gate change, no
   TS-client versioning) and leaves the `full-reload` frame untouched —
   a reload clears state by definition.
2. **The policy is a trait, not a constant.** The F18 `Probe` seam
   pattern: the default policy is spec-verbatim, and a custom policy can
   flip any class (the never-preserve pin proves injectability is load
   bearing). The verdict is deterministic wire contract — the same plan
   must always produce the same verdict.
3. **DOM-state preservation is a client mechanism, not an adapter
   runtime.** The spec lists per-adapter refresh runtimes (React
   Refresh, Vue `__VUE_HMR_RUNTIME__`, Solid `solid-refresh`); the
   snapshot/restore layer under them is adapter-independent and covers
   the raw form state those runtimes do not own (uncontrolled inputs,
   textareas, selects, focus).
4. **The R02 sketch's Live cells stay as F05 landed them.** Not an E01
   delta — recorded here because the same substrate pass touched the
   matrix: see the V2-R02 record below.

**What is deliberately deferred:**

- The Playwright acceptance run (`hmr-state`: type into an input, edit
  the component, assert the value survives; edit CSS, no navigation) —
  the in-sandbox evidence is the 7-pin Rust wire suite, the 9-test TS
  client suite and the `hmr_state` walkthrough example; the browser-run
  proof belongs on a browser host (the same deferral every prior E01
  phase recorded).
- Island-hydration race hardening for the boot-time stash restore: the
  double pass (microtask + 50 ms timer) is idempotent by assignment, but
  a late-hydrating island that rebuilds a field after the second pass can
  still lose the restored value; tracking the adapter mount promise is
  the follow-up.
- A config surface for the policy (`dev.hmr.preserve: 'never'` etc.) —
  the trait is the seam; no config key until a user asks.

## V2-R02 capability matrix v2 — deltas and deferrals (substrate step)

**What lands.** `crates/adapters/src/lib.rs`: the fixed-size
`row([(Capability, FeatureSupport); 8])` table becomes `support(target,
capability)`, a total function over the private `Class` enum
(`Native`/`NodeLike`/`Static`/`Edge`/`Serverless`) — one arm per
capability with an exhaustive inner match on the class, so the compiler
proves both dimensions (a new `Capability` variant is a compile error
until its row lands; a new `Target` class is a compile error until every
row covers it). `Capability::ALL` (declaration order: v0.1 first — the
`BTreeMap` iteration order is observable — then `Live`, then the v0.2
set) gains `MixedIslands`, `Resume`, `Skew`, `DeltaDeploy`, `RustFns`,
`Search`, `Offline`, `Brownout`, `Permissions`, `Attestation`.
`capability_matrix` is now a fold of `support` over `ALL`, so the map and
the function cannot disagree. Every new `Degraded` cell documents its
reason in `degradation` (the `every_degraded_cell_documents_its_reason`
pin enforces the obligation matrix-wide). The §18 refusal rule
(`check_requirements`) extends to the v0.2 server-manifest `requires`
keys (`live`, `mixedIslands`, `resume`, `skew`, `deltaDeploy`, `rustFns`,
`search`, `offline`, `brownout`, `permissions`, `attestation`): an
Unsupported capability refuses the build (`ADP-CAP`), a Degraded one
proceeds with the documented string. Pins in
`tests/matrix_v2_tdd.rs`: the v0.1 golden rows byte-identical, the v0.2
rows per the spec table, every target × every capability, the refusal
rule over the new keys, and the `ALL` order stability.

**Spec deltas:**

1. **The v0.1 rows win over the R02 sketch's Live arm.** The sketch wrote
   `(Static | Edge, Live) => Unsupported`, but the landed V2-F05 table —
   and `tests/live_matrix_tdd.rs`, the pin D39 protects — has Edge Live =
   Degraded with the poll fallback. Byte-identical beats the sketch; the
   comment in `support` records the ruling.
2. **Row shape: per-capability arms with inner class matches, not the
   sketch's flat `(Class, Capability)` arms.** The sketch's shape trips
   `clippy::match_same_arms` (identical bodies, even non-adjacent) and
   `clippy::unnested_or_patterns` under the pedantic sweep; the nested
   shape keeps both compiler-proved totality properties and is what the
   lint-clean build runs.
3. **The refusal rule extends through the server manifest's `requires`
   object only.** The spec also names config-key refusals (`live: true`
   on static → `AST-LIV-001`, `search` on edge → `AST-SRC-001`,
   `fonts.subset` + dynamic routes → `AST-FNT-001`, rust fns →
   `RFN-002`); those keys live in the config/CLI layer and are the
   follow-up half of R02, not this substrate (the adapters crate cannot
   see `oxide.config.ts`).
4. **The v0.2 rows describe target classes, not shipped features.** The
   matrix is the target-class contract (D39's "add capabilities without
   ever editing rows"); features whose WPs are still todo (F01, F08, F09,
   F10, F11, F14, F19) carry their row from day one so no row is ever
   edited later. The F19 caveat the spec itself writes (Permissions:
   "Native is Full only if F19 passes") is recorded in the
   `Capability::Permissions` doc — a documented caveat, not a silent one.

**What is deliberately deferred:**

- The config/CLI half of the refusal rule (the `AST-*`/`RFN-*` codes on
  config keys) — needs the config surface, and lands with the features
  that introduce the keys.
- `oxide doctor`'s per-capability column and `oxide build --explain`'s
  matrix row — presentation work on the CLI side; the matrix output is
  already machine-readable via `capability_matrix`.
- The generated docs table (`doc_table_matches_matrix`, E11) — the
  docs-site generator does not consume the matrix yet; the README-level
  tables stay prose.

## V2-R02 config-key refusal half — the seam, the live codes, and the F09-gated search arm (2026-11-11)

**What the spec asks.** §2776's refusal rule: `check_config(cfg, t) →
Vec<Diagnostic>` — for each enabled feature, an `Unsupported` cell returns
an error diagnostic with the registry code (`live` → `AST-LIV-001`,
rust functions present → `RFN-002`, `fonts.subset: 'used'` with dynamic
routes → `AST-FNT-001`, `search` on edge → `AST-SRC-001`), a `Degraded`
cell a warning with the documented degradation string.

**What lands.** The dependency direction the V2-R02 record already ruled
(config → adapters, never the reverse): `crates/adapters/src/refusal.rs`
owns the seam — the `ConfigFeatureReport` trait (the feature surface the
config layer reports) and `check_config(report, target)`, pure and
infallible, every verdict re-stating `support(target, capability)` as a
`Diagnostic` (the matrix stays the single source of truth — the seam is a
consumer, never a second opinion). `crates/config/src/refusal.rs` maps the
typed config into the report: `fonts.subset: ['used']` is the C08 key's
spelling, and the three project facts (`ProjectFacts`: live routes,
`*.server.rs` presence, non-static routes) ride verbatim from the driver —
the CLI build computes them from the scanned route tree (`RouteEntry::live`,
the effective §10.1 render modes) plus a `**/*.server.rs` source walk, and
refuses before any deploy-artifact work. `oxide doctor`'s per-capability
column and `oxide build --explain`'s matrix row land in the same diff (the
R02 record's deferred presentation half), over `Capability::as_str()`
kebab-case ids.

**Spec deltas:**

1. **`check_config` takes the feature report, not `&Config`.** The spec's
   sketch signature hides two inputs no config key carries: route-level
   `live` opt-ins (F05: per-`page.server.ts`, not per-config) and rust-fn
   presence (F08: a project fact). The trait splits "what the config layer
   owns" (keys) from "what only the driver can know" (facts) — the adapters
   crate still never sees `oxide.config.ts`.
2. **The fonts rule is target-independent in the landed reading.** The
   registry row ("`fonts.subset: 'used'` with non-static routes") names no
   target: glyph-usage subsetting needs all text known at build, so the
   refusal fires on every target whenever a non-static route exists (C08
   §1707: "allowed only when every route is `static`").
3. **`AST-SRC-001` has no arm yet — the search key does not exist.** F09's
   `search: { collections: … }` config key is the "search configured"
   surface (§16.5's collection indexes are static JSON — the registry's own
   remedy for unsupported targets, so they must NOT refuse). The trait
   gains a `search_configured` method and `check_config` gains its arm
   when F09 introduces the key — additive, the D39 rule applied to the
   refusal seam. The `warns_degraded_search_on_node` pin rides with it.
4. **Degraded warnings carry the refusal code at `Warning` severity**
   (`AST-LIV-001` on serverless/edge), not a new code — the registry entry
   ("`live: true` on a target without long-lived connections") already
   covers the degraded case, and V2.K defines no warning-specific codes.

**What would change it.** F09's key lands (`search_configured` + its arm +
the spec's `warns_degraded_search_on_node` pin); F08's loader flips
`rust_fns_present` from a walk to the real tooling-pool compile step
without touching the refusal layer.

## V2-F09 search config key + the AST-SRC-001 arm — the config-key refusal rule is complete; E11's docs-table seam realized (2026-11-11)

**What the spec asks.** §2776's refusal rule names `search` on edge →
`AST-SRC-001` as the fourth config-key code, and Appendix V2.L types the
key (`search: { collections: string[]; fields?: { title?: number;
body?: number }; snippet?: number; languages?: string[] }`). E11 names
`doc_table_matches_matrix` — "generated table equals the function output" —
as the red-first pin over the capability table.

**What lands.**

- **The `search` key, typed and schema'd from day one**
  (`SearchConfig`/`SearchFieldsConfig` in `crates/config/src/lib.rs`, the
  `search_schema()` fragment + the regenerated
  `schemas/oxide-fw-config.schema.json`, the key added to the
  typo-suggestion validator's `KNOWN_KEYS`). The full V2.L shape lands
  (collections, field boosts, snippet, languages) so F09's engine
  consumes the query knobs without a second schema churn; today the key
  is inert — with no `collections` configured nothing indexes and the
  refusal reports the surface as unconfigured.
- **The refusal arm**: `ConfigFeatureReport::search_configured()` (a
  required method — the compiler walks every impl and fixture) and the
  `check_config` search arm, consulted in capability declaration order
  (live → rust fns → search → the target-independent fonts rule). Edge
  refuses with the registry code + fix text ("Use static JSON search or a
  Binary or Docker target"); Node/serverless/static warn with the
  documented degradation string. `search_configured` maps ONLY
  `search.collections` — the §16.5 collection indexes are static JSON,
  the registry's own remedy for unsupported targets, so their presence
  never refuses (the R02 record's reading, unchanged).
- **The E11 docs-table seam** (`feat/matrix-doc-table`):
  `capability_markdown_table()` in `crates/adapters/src/lib.rs` — the §18
  matrix as a deterministic markdown table (rows = `Target::ALL`, columns
  = `Capability::ALL`, cells `F`/`D`/`U`), a fold of `support` like
  `capability_matrix`. The crate README embeds the exact output and
  `tests/doc_table_tdd.rs::doc_table_matches_the_function` fails when
  they drift — the E11 pin realized at the crate-docs level. A
  `doc_table` criterion bench and the extended `capability_matrix`
  walkthrough ride along.

**Spec deltas:**

1. **The key lands before the engine.** F09's tantivy WP (`crates/search`,
   the index/serve/client halves) is untouched; the config surface and the
   enforcement arm land first — the same shape RFN-002 shipped under (the
   enforcement the registry already names, live from day one; the loader
   is a later half). The V2.L key shape is complete so the engine needs
   no schema or typed-config change.
2. **No load-time validation for the query knobs.** The spec attaches no
   DSP code to `search` (unlike `live`'s DSP-111, `stream`'s DSP-112,
   `brownout`'s DSP-113); inventing one would add a registry row the
   spec does not define. The typed parse rejects wrong shapes
   (`deny_unknown_fields`, `Option` boosts), which is all the spec asks
   today.
3. **`search_configured` is a required trait method, not a defaulted
   one.** A default would let a future report silently miss the surface;
   the required method turns every impl (the config crate's
   `FeatureReport`, the test fixtures, the walkthrough examples) into a
   compile-forced decision — the same compiler-proved shape the matrix
   rows use.
4. **The E11 table embeds in the crate README, not a docs-site
   generator.** The `capabilities.md` generator (E11's full WP) does not
   exist yet; the README is the docs surface the repo actually ships and
   gates, so the generated table lands where the drift gate can hold it.
   The docs-site generator consumes the same function when it lands.

**What would change it.** F09's engine lands against the unchanged typed
key (the `collections` names must match `content.collections` entries at
build time — the engine's validation, not the refusal's); E11's
docs-site generator replaces the README embed with `capabilities.md`
rendered from the same function.

## V2-E11 the reference-docs generator + the error-code registry — `oxide docs generate` writes docs/reference/ from the linked crates; the V2.K call-site gate is a test (2026-11-11)

**What the spec asks.** §V2-E11: "`oxide docs generate` writes
`docs/reference/`: `config.md` from the JSON Schema, `errors.md` from
the error code registry (Appendix V2.K, kept as `docs/errors.toml` and
checked against `rg 'OxideError|err("' call sites by a test), `cli.md`
from the `clap` command tree, `capabilities.md` from
`oxide_adapters::capability_matrix()` ... CI job `docs-check`
regenerates and fails on `git diff --exit-code`." Acceptance: "adding a
config key without a schema description fails `docs-check`."

**What lands.**

- **M1 — the generator seam** (`feat/docs-generate-seam`):
  `crates/cli/src/docs_gen.rs` owns the `ReferenceGenerator` trait (a
  stable `path()` under `docs/reference/` and a deterministic
  `render()`), the static registry (`generators()`), `generate_all()`
  and the read-only `check()` that reports `DOC-003` findings the way
  `check_config` reports refusals. Three pages: `capabilities.md` (a
  fold of `capability_markdown_table` + the `degradation` reasons — the
  matrix stays the single source of truth), `config.md` (a walk of
  `config_schema()`, one bullet per flattened key, sorted — refusing
  with `DOC-001` when any leaf lacks a description), `cli.md` (the live
  clap tree: about lines, argument tables, value-enum variant rows).
  `oxide docs generate` (`Command::Docs` + `DocsOp::Generate`) writes
  the pages under `<root>/docs/reference` (root canonicalized at
  entry, the CONVENTIONS rule); `--check` re-renders in memory and
  exits 1 on drift. The committed pages are pinned by
  `tests/docs_tdd.rs::check_passes_on_the_committed_pages`; the
  `docs-check` CI job runs the same gate. The criterion bench
  (`benches/docs.rs`, `bench-util`) and the `examples/docs_generate.rs`
  walkthrough ride along.
- **M2 — the registry seam** (`feat/errors-registry-page`):
  `docs/errors.toml` at the repo root — 197 entries, one per stable
  code: the message (authored from the call sites, format placeholders
  kept verbatim), the documented `fix` where one exists, and
  `spec_only = true` for the seven V2.K additions whose WPs have not
  landed (`CMP-NAT-001`, `RFN-001`, `RPL-002/003`, `OX-SRC-001`,
  `BLD-409`, `PRM-001`). `parse_registry` is a strict parser for the
  spec's entry format (`DOC-002` names the offending line: unknown
  keys, non-bare headers, keys outside tables, duplicates, missing
  messages). `errors.md` renders the registry (landed codes first,
  spec-only marked), and `tests/errors_registry_tdd.rs` is the V2.K
  call-site gate: every code-shaped literal in production sources
  (tests/benches/examples/fuzz excluded) must have a non-spec-only
  registry row.

**Spec deltas:**

1. **The registry parser is hand-rolled, not the `toml` crate.** The
   workspace has no TOML dependency (the config story is
   static-extraction + serde; the deploy half writes wrangler.toml as a
   string). The entry format the spec pins is a strict subset of TOML
   (`[CODE]` bare-key tables + three keys), so a ~90-line strict parser
   with `DOC-002` diagnostics keeps the tree dependency-free — the
   CONVENTIONS rule ("prefer implementing it yourself") applied to the
   registry. The file stays valid TOML.
2. **`check` compares bytes, not `git diff`.** The spec's docs-check
   regenerates and fails on `git diff --exit-code`; the landed shape is
   `docs generate --check` — render in memory, compare against the
   committed pages, exit 1 — so the gate needs no git and no dirty
   worktree, and the CI job is one `cargo run`. Same failure mode, one
   fewer moving part.
3. **The registry is embedded via `include_str!`, not read at
   runtime.** The pages stay pure (no `--root`-dependent render), and
   the binary documents the registry of the exact crates it was built
   from. Consequence: a packaged `oxide-fw-cli` crate must ship
   `docs/errors.toml` (path `crates/cli/src/../../../docs/errors.toml`)
   — moot until the crates.io publication step lands.
4. **`endpoints.md` and `plugin-compat.md` are deferred.** The V2.J
   endpoint map names future WP endpoints (`/_oxide/search`,
   `/_oxide/nav`, `/_oxide/csp-report`, the E08 dev surfaces) — a page
   generated today would pin half-invented rows. The E05-dependent
   plugin-compat page waits for E05. The docs-site Oxide app
   (`examples/docs`) stays the remaining E11 half.

**What would change it.** The docs-site app consumes `generate_all()`
directly (dogfooding F09's search over the pages); `endpoints.md` lands
when the V2.J table's open rows do (F09's engine, E04's CSP report,
E08's dev surfaces); the registry harvest widens to the spec's
`rg`-shaped gate over TS call sites when the client gains its own codes.

## V2-E04 SRI + Trusted Types + CSP report-only — the manifest digests, the header matrix and the violation sink (2026-11-11)

**What the spec asks.** "At client bundle end, compute `sha384-<base64>`
for every emitted JS and CSS chunk and store it in `client.manifest.json`
as `integrity: { file: value }`. The head injector adds
`integrity='...' crossorigin` to `<script type=module>`,
`<link rel=modulepreload>` and stylesheet links for chunks (not for
dev)." A mismatch case documented. Trusted Types (`off | report |
enforce`) with the CSP directives and the client-side `tt.createHTML`
policy; `security.csp: 'report-only'` sending
`Content-Security-Policy-Report-Only` + `report-to` with a
`POST /_oxide/csp-report` sink (both formats, 20/min/IP, structured
no-cookie logging, 204).

**What lands.** The SRI half end-to-end: `crates/bundler/src/sri.rs`
(`sri_digest` over the exact emitted bytes — the chunk path digests
memory it already holds, the rolldown path reads the persisted files
back; a file that fails to read is skipped with a `tracing::warn!` and
the manifest carries fewer entries), `ClientManifest.integrity`
(`#[serde(default)]`, so a pre-E04 manifest parses), the boot-tag stamp
in the CLI (`boot_script_tag`, one definition site for the prerender
and live-SSR injectors, gated by `security.sri` — default on; the dev
`/@boot/` virtual module has no digest, so dev output is byte-identical
without a dev/prod key split), and `render`'s `Injector.integrity` map
for the modulepreload/stylesheet links (snapshot-pinned; an empty map
keeps every link byte-identical). The CSP half:
`server::csp_headers_for` is the one definition site for the whole
strict/hash/report-only × Trusted Types header matrix, and
`POST /_oxide/csp-report` (`csp_report_response`) accepts both the
classic `application/csp-report` form and the Reporting API's
`application/reports+json`, rate limited 20/min/IP on the
trusted-proxy-aware peer key, with a 64 KiB body cap (413), a 415 for
unrecognized content types, a 400 for unparseable JSON, a 405 for
non-POST and the structured `tracing` event over a whitelist of fields
(`effectiveDirective`, `sourceFile`, `lineNumber`, `disposition`) — no
cookies, no raw body. The typed `security.sri`/`security.trustedTypes`
keys are schema'd with descriptions (the E11 DOC-001 gate) and
load-time-validated (`CFG-SEC` names a bad spelling with the valid
modes as the fix).

**Deltas and deferrals.**

1. **The link-integrity seam is wired to the Injector — CLOSED since
   (the head producer landed, see the V2-E04-tail section below).** The
   spec reads "the head injector adds integrity to `<script
   type=module>`, `<link rel=modulepreload>` and stylesheet links".
   The landed `Injector.preload`/`css_links` fields had no production
   producer — the prerendered shells take their head from the adapter
   (and the adapter's own `<link>` output is not manifest-aware). The
   seam is complete and pinned (the map attaches through
   `PrerenderArgs.manifest_integrity`, the links resolve
   `/client/<file>`, `client/<file>` and bare-name URLs and emit the
   attributes byte-exactly). The adapter-level head producer
   (`bundler::head::ManifestHeadProducer`, wired through the CLI's
   prerender paths via `filter_head_links`) now fills
   `preload`/`css_links`; modulepreload is producer-owned and the
   adapter's own head emission stays authoritative (the filter dedupes
   by href), so no shell link is emitted twice.
2. **The boot-side Trusted Types policy is the client half.** The spec
   pins the `tt.createHTML` policy definition in `packages/oxide` and a
   lint test (`rg 'innerHTML' packages/oxide/src` matching only lines
   using `tt.`). That is TS work in the client runtime — the Rust half
   (the typed mode, the schema, the CSP directives, the load-time
   validation) is complete and inert until the client policy lands, the
   same RFN-002 pattern the `search` key used.
3. **The mismatch case.** A digest that does not match its chunk is a
   client-side failure mode: the browser refuses to execute/load the
   resource and reports it as a console error (net::ERR_… integrity
   mismatch) — the failure is closed, not silent. Server-side, the
   digests are computed from the same bytes the bundler just wrote (or
   read back), so a stale digest can only appear when the manifest and
   the chunk files disagree on disk — exactly the corrupted-deploy case
   the feature exists to catch. `oxide verify` (F20) is that serve-side
   check — CLOSED since (see the V2-E04-tail section below): the verify
   walk re-digests the manifest's `integrity` map against the bytes on
   disk and fails closed (`VRF-INTEGRITY`) on a mismatch, a missing
   file or a corrupt manifest; a non-client build skips loudly.
4. **`csp-report` rejections are strict where the vitals beacon is
   lenient.** The vitals beacon answers a malformed body with a silent
   204 (the browser never reads the response; a 4xx would only add
   noise). The CSP sink answers 415/400/413/429 — the endpoint is
   unauthenticated and public, so a garbage body is an attacker or a
   broken client and should be visible in the logs, not silently
   drained.

## V2-F13 learned prefetch — the nav-graph substrate, the config key and the client half — deltas and deferrals (substrate step) (2026-11-11)

**What the spec asks.** §V2-F13: the client router adds `x-oxide-nav-from:
<route id>` to the `/__data` fetches it issues for navigations; the server
keeps a `NavGraph` of aggregate transition weights (7-day half-life, the
k-anonymity floor at 20 observations, edges are route ids — never URLs with
params, no user identifiers), `GET /_oxide/nav?from=<route>` answers the top
3, the graph flushes to `.oxide/nav-graph.json` hourly and loads at start,
and the client prefetches data and chunks for the edges at or above
`prefetch.minProbability` (default 0.35) after idle — max 3 outstanding, one
new prefetch per 2 s, cancelled on navigation, plus a Chromium-only
speculation-rules hint for the top edge.

**What lands.** The server substrate end-to-end
(`feat/learned-prefetch-navgraph`): `crates/server/src/navgraph.rs`
(`NavGraph` — the `DashMap<(route id, route id), f64>` weight table with
`observe`/`decay`/`top`, the throttled lazy decay, the 50k-edge cap,
`sanitize_edge_route`'s skip-not-fold posture, `merge_from`/`persist` —
the atomic tmp+rename snapshot, malformed content ignored — and
`spawn_flusher`); the pipeline's native-prefix observation arm (the data
dispatch is never answered by it) and the `/_oxide/nav` endpoint
(`nav_response_from_query`: `{from, edges: [{to, probability}]}`, `?limit=`
capped at 32, 400 on a missing/empty `from`, 405 + `allow` on a non-GET,
and the metrics endpoint's "not configured" 404 when no graph is attached);
the typed `prefetch` config key (`learned` default off — the inert-defaults
contract the `live`/`stream`/`brownout` sections set — and `minProbability`
default 0.35) schema'd with descriptions and load-time-validated
(`DSP-115`, the DSP-110..114 successor — a quoted `"true"` or a
`minProbability` outside 0.0–1.0 fails the load before the typed parse);
the seventh `window.__oxide` field (`prefetch:true`,
`hydration_bootstrap_with_prefetch` + `hydration_bootstrap_prefetch_no_build`
+ `Injector::with_prefetch` — `false`, the default, keeps every emitted
byte identical), threaded from the typed config through the CLI's three
prerender emit paths and the `oxide start` nav wiring (the graph is
constructed, the snapshot merged at start, the flusher spawned); the
client half (`packages/core/src/client/prefetch.ts` — the connection gate,
`pickEdges`, the manifest-resolved route-id headers, `edgeToDataPath`, the
idle-gated loop; the router stamps the nav headers on navigation
`/__data` fetches and never on prefetches; `boot.ts` starts the loop when
the bootstrap advertises `prefetch:true`). 22 server TDD pins + 8 config
pins + 7 render bootstrap pins + 1 CLI wire-up pin + 14 TS pins; the
`navgraph` criterion bench (`observe`/`top`/`decay`/`persist` behind
`bench-util`) and the `navgraph.rs` walkthrough example ride along.

**Deltas.**

1. **Route ids ride two headers.** The spec's `observe(from, to)` says
   edges are route ids, never URLs with params — but the `/__data?route=`
   value is the concrete path (`/blog/a`), not the id
   (`routes/blog/_slug_/page`). The client therefore stamps BOTH ids:
   the spec's `x-oxide-nav-from` for the source and `x-oxide-nav-to` for
   the target, resolved against the route manifest (`routeIdFor`); both
   must be present and sane or nothing is observed. Storing the raw path
   would fragment the graph per-instance (`/blog/a`, `/blog/b` — separate
   edges that never clear the k-anonymity floor) and leak URL params into
   the aggregate.
2. **Garbage is skipped, not folded.** The vitals beacon folds a
   malformed route label into `_other`; a nav edge must not — a folded
   `_other` target would accumulate an artificial popular transition.
   `sanitize_edge_route` refuses anything outside the route-id alphabet
   (plus the dynamic-segment brackets) and the observation is dropped.
3. **The data prefetch covers param-free edges only.** The spec reads
   "prefetches data and chunks for edges with probability at least 0.35";
   a param route id has no single concrete `/__data` path to fetch (the
   id→path inversion is one-to-many), so `edgeToDataPath` inverts only
   param-free ids (`routes/about/page` → `/about`, `routes/index/page` →
   `/`) and param edges warm their chunk lists (which ARE id-addressable
   through the manifest) without a data fetch. Closing the gap needs the
   §8.3 typegen map on the wire — a follow-up slice, not scheduled here.
4. **Decay is lazy, not timed.** The spec's `decay(now)` runs on access
   (throttled to once per minute of wall clock on `observe`, always on
   `top` and `persist`) instead of by a background timer — an idle
   server's graph is cost-free, and the explicit `decay(now)` stays
   testable with an injected instant. The hourly flush decay runs first,
   so the snapshot carries current weights.
5. **`minProbability` rides the module default on the client.** The
   bootstrap advertises `prefetch:true` (the switch) but not the
   per-project threshold; the client uses the spec's 0.35 until the
   bootstrap carries `minProbability` too (a one-word addition when a
   consumer asks for it).

**Deferrals.** None F13-specific: the endpoint, the observation, the
snapshot round trip and the client loop are all wired end-to-end. The
conformance-suite assertion (the F-WP obligation) rides the same wire the
Playwright smoke specs drive; adding a dedicated spec is the R01 pass.

## V2-F09 the built-in search engine — the engine, the serve arm and the build-side emission landed; the client matcher is the remaining half (2026-11-11)

**What the spec asks.** §V2-F09: "`oxide build` indexes content
collections; the server answers queries natively; static exports ship a
compact JSON index with a tiny client matcher." Files
`crates/search/{Cargo.toml,src/lib.rs,src/schema.rs,src/build.rs,src/query.rs}`
(feature `search`, default off in dev builds, on in release),
`packages/oxide/src/search.ts`, and the hook in `emit/search.rs`; index
`title` with boost 3.0 and `body` with 1.0, fuzzy distance 1 on terms of
length 5 or more, snippets via tantivy's snippet generator; the
`/_oxide/search?q=&limit=&tag=&lang=` endpoint with the L1 (60 s, keyed by
the normalized query) and the per-IP rate limit (20 per 10 s); the static
fallback `dist/search/static.json` with prefix terms of length 2 to 4;
CAS-cached by the content hash of the collection; the acceptance: a
5,000-document corpus builds under 5 s and answers p99 under 5 ms.

**What lands.**

- **The engine crate** (`feat/search-engine-substrate`): `crates/search`
  — `schema::build_schema` (the spec's six fields verbatim: `id`/`url`/
  `tags`/`lang` stored strings, `title`/`body` stored text with
  frequencies and positions, all indexed under the `en_stem` tokenizer),
  `query::SearchDocument` (the ingest contract) +
  `SearchEngineOptions::from_config` (the typed `SearchConfig` knobs:
  `fields.title`/`fields.body` boosts default 3/1, `snippet` default 160,
  `languages` default `["en"]`), `build::TantivyIndex`
  (`build_in_ram`/`build_in_dir`/`open_read_only` — the serve side mmaps
  the segments, no writer exists at runtime; `content_hash` the blake3
  CAS key, order-insensitive), the query path (one should-clause per
  token per field with the configured boosts; `FuzzyTermQuery` at
  distance 1 for terms of length ≥ 5, exact term queries for the shorter
  ones; the `tag`/`lang` filters as required term clauses; snippets via
  tantivy's `SnippetGenerator` re-prefixed to `<mark>`/`</mark>`), the
  object-safe `SearchIndex` seam (the Full engine +
  `static_index::StaticMatcher`, the Degraded fallback) and
  `endpoint::QueryCache` (60 s TTL keyed by the normalized query + the
  two filters, cap-bounded LRU, injectable clock — the deterministic-tests
  rule). Pins: `crates/search/tests/search_tdd.rs` 27 (schema shape,
  ingest refusals, CAS hash, dir-build→reopen, title-beats-body, boost
  inversion, empty-query refusal, fuzzy budget, tag/lang filters,
  `<mark>` snippets, the static shape/round trip/matcher, the 20-query
  top-3 parity, the cache TTL/cap/normalization). Bench:
  `crates/search/benches/search.rs` (`search_build/{100,5000}`,
  `search_query` ×4 shapes, `search_static_export/1000`). Example:
  `crates/search/examples/search_engine.rs`.
- **The serve arm** (`oxide-fw-server/search`): `server::search_endpoint`
  — `SearchEndpoint` (the read-only engine + its L1; the cache key covers
  the filters so a tag-narrowed answer is never served unfiltered) and
  the pure `search_response` renderer; the pipeline's native-prefix arm
  (`GET /_oxide/search`, the not-configured 404 naming the contract, the
  405 + `allow: GET`, the 20-per-10-s token bucket — the endpoint's own,
  never shared with OG/csp-report — and `limit` capped at 100). Pins:
  `crates/server/tests/search_endpoint_tdd.rs` 8.
- **The build-side emission** (`oxide-fw-cli/search`): the CLI's
  `emit_search_engine_index` (each configured `search.collections` name →
  the scanned collection's entries mapped by
  `search_emit::collection_document` — id `<collection>/<slug>`, the
  default `/<collection>/<slug>` URL shape, frontmatter title/tags/lang
  with the `search.languages` fallback, the conservative
  `strip_markdown`) into `dist/search/index` +
  `dist/search/static.json`, the blake3 CAS marker (`.hash`) skipping an
  unchanged corpus, and `SRC-106` failing the build on a typo'd
  collection name; the serve-side `build_search_endpoint` wire-up in
  `start_server_inner_full` (best-effort open — a missing or corrupt
  index is a loud `tracing::warn` and the not-configured 404, never a
  serve-start failure); the release binaries build `--features search`.
  Pins: `crates/cli/src/tests.rs` 5 (the emission pin, the CAS-skip pin,
  the SRC-106 refusal, the serve wire-up, plus the §16.5 projection
  pinned byte-identical alongside).
- **The registry**: `SRC-101`–`SRC-106` in `docs/errors.toml` + the
  regenerated `docs/reference/errors.md` (the docs-check gate).

**Spec deltas.**

1. **The §16.5 content projection is NOT superseded away.** The spec
   says the WP "supersedes" the `emit/search.rs` JSON index; the
   projection is also the loader-side contract (`getCollection` reads
   `dist/content/<name>.entries.json`), so the F09 index is additive and
   the serve side prefers it — the built-in search UI hits
   `/_oxide/search`, the loader surface keeps its files. (The spec's
   `emit/search.rs` path is itself stale: the symbol is
   `emit_content_search_indexes` in `crates/cli/src/lib.rs`.)
2. **The analyzer set ships one stemmer.** `search.languages` is typed,
   schema'd, threaded through `SearchEngineOptions` and filter-able via
   the `lang` field, but the title/body analyzer is `en_stem` (simple →
   length cap → lowercase → English stem). Per-language analyzers need
   per-document field routing (a corpus-level change); German and other
   non-ASCII text still matches through the lowercase layer, and the
   degraded matcher is exact-term + prefix by construction.
3. **The URL shape is the default collection-page form.** Content
   collections carry no route template in the typed config, so the
   indexed `url` is `/<collection>/<slug>`; the client/UI half maps ids
   onto real routes when it lands (the static matcher never claims
   otherwise — `url` is the hit's link target).
4. **The CAS is a dist-local marker, not the TieredCache.** The corpus'
   blake3 lives in `dist/search/.hash` next to the artifact (the A04
   `dist/build-id` pattern): the L2 disk cache stores big blobs keyed by
   build id, while the search rebuild decision is a 32-byte compare that
   must survive `oxide dev` restarts without a cache open.
5. **Fuzzy-only queries highlight nothing.** The snippet generator is
   built from a plain term query over the query's tokens (tantivy's
   `query_terms` walk does not reach through `BoostQuery`/
   `FuzzyTermQuery`); a distance-1-only match still returns the doc with
   the body's leading fragment, un-highlighted. The engine-side
   `SRC-104` 400 and the punctuation-only empty response are pinned.

**Deferrals.**

- **The client matcher** (`packages/oxide/src/search.ts` — the
  `search(q) => Promise<Hit[]>` import, the lazy `static.json` load, the
  ten-line scorer, the `window.__oxide.search === 'server'` fork and the
  TS-side parity suite) is the remaining half — the Rust-side parity
  (`static_matcher_top3_matches_the_engine_on_twenty_queries`) pins the
  export's self-sufficiency until it lands. The spec's Search matrix row
  stays: Full on Binary/Docker (the engine), Degraded on
  Node/serverless/static (the static export), and the TS matcher is what
  ships the Degraded cells to browsers.
- **The dev-watch re-emission.** `oxide build` emits the index;
  `oxide dev` re-emits the §16.5 projection on content change but not
  the F09 index yet (the running server holds the opened index — the
  reload path is tantivy's `IndexReader::reload` behind the
  `SearchIndex` seam, the follow-up slice).
- The benchmark-acceptance numbers (`5 s`/`5 ms` at 5,000 docs) are
  machine-relative; the criterion groups (`search_build`,
  `search_query`) record the local baseline.

## V2-E04 tail — the adapter-level head producer + the verify integrity walk (2026-11-11)

**What the spec asks.** The same §V2-E04 clause the parent section quotes:
the head injector adds `integrity` to `<script type=module>`,
`<link rel=modulepreload>` and stylesheet links — which needs something to
actually produce those links — and "a mismatch case documented" (the parent
section's deltas 1 and 3, both closed by this pass).

**What lands.** The head half (`feat/head-producer-integrity-verify`):
`crates/bundler/src/head.rs` — the `HeadProducer` seam (`name()` +
`head_for(route)`, object-safe, the `PageRenderer`/`LoaderRunner` trait
conventions) and `ManifestHeadProducer`, the manifest-driven implementation:
the three-shape route-key resolution the CLI shim's `inject_route_css` uses
(exact, `+ /page`, the `_param_` sanitized dynamic form), critical-first
preload order preserved, first-seen-order dedup, URLs spelled
`/client/<file>` so every one resolves to a digest in the
`integrity` map. The CLI wire-up: `cmd_build` constructs the producer from
the typed manifest (a pre-E04 manifest that fails the typed parse degrades
to no links with a loud `tracing::warn!`, never a build failure), and every
prerender path — `prerender_targets`'s generateParams arms plus the
`Absent` arm's direct writes — fills the render `Injector`'s
`preload`/`css_links` through `filter_head_links`, which dedupes against
the hrefs the adapter's shell already carries. Division of labor (the
design decision): modulepreload is producer-owned (nothing else emitted
it), the adapter's own head emission stays authoritative for the
stylesheets it renders, so the producer **completes** the shell's head
instead of duplicating it; the no-renderer placeholder path (a project
without `componentFor`) gets its manifest stylesheets for the first time.
The verify half: `crates/bundler/src/sri.rs::verify_integrity_map` — the
strict re-digest walk (`IntegrityReport`: checked / missing / mismatched,
findings in manifest key order) — and the `oxide verify` step
(`integrity_verdict`): a clean walk prints `integrity: N files verified
(sha384)`, a mismatch/missing/corrupt manifest fails closed with the
`VRF-INTEGRITY` registry row (the findings listed above the diagnostic), a
non-client build or a pre-E04 manifest (no `integrity` key) skips loudly —
the signature check's skip verdict, not a silent pass.

**Status.** Closed for the server-side loop the parent section named. The
remaining E04 work is the client Trusted Types half (`packages/oxide`'s
`tt.createHTML` policy + the `rg` lint test) — TS work, unchanged.

**Deltas and deferrals.** The live-SSR path still takes its head from the
adapter stream (body-side tags ride `inject_before_close`); adopting
`Injector::streaming()` for the live head is a follow-up slice, not
scheduled here. The rolldown backend records `preload: vec![entry]` —
manifest-fed prerender shells of rolldown-built projects carry one
modulepreload until its manifest walk widens (tracked in the bundler, not
the producer: the producer is a faithful projection by design).

## V2-F09 tail — the dev-watch re-emission + the hot-reload seam (2026-11-11)

**What the deferral asked.** The parent F09 record deferred the dev-watch
half: "`oxide build` emits the index; `oxide dev` re-emits the §16.5
projection on content change but not the F09 index yet (the running
server holds the opened index — the reload path is tantivy's
`IndexReader::reload` behind the `SearchIndex` seam, the follow-up
slice)." This pass closes it (`feat/search-hot-reload`).

**What lands.** The engine half (`crates/search/src/hot.rs`):
`SwappableSearchIndex` — the CLI's `SwappableRouteTree` snapshot-handle
pattern plus the disk-driven reload. `try_reload` fingerprints the index
directory's `meta.json` bytes: the emitter's CAS skip leaves the
directory untouched, so an unchanged corpus is byte-identical and the
reload is `Unchanged` (no mmap thrash, no cache churn); a rebuild
commits fresh segment UUIDs, so the bytes differ and the rebuild is
opened read-only and swapped in atomically (`Swapped { docs }`); a
missing, mid-wipe or corrupt directory refuses with the engine's
`SRC-101` and the previous snapshot keeps serving — a failed reload
never degrades the running answers. The seam goes through the object
surface: `SwappableSearchIndex` itself implements `SearchIndex`, and
`SearchIndex` gains a defaulted `generation()` — the corpus identity the
endpoint's L1 cache folds into its keys, so a swap repartitions every
key (a pre-reload hit can never survive) while an unchanged corpus keeps
every live entry valid (`TantivyIndex` and `StaticMatcher` inherit the
0 default; they never replace their corpus in place). The wire half
(`crates/cli`): `cmd_dev` emits the F09 index at boot (next to the
§16.5 projection, so `/_oxide/search` answers without a prior
`oxide build`), the watcher's content-change branch and the `--once`
hook call `refresh_search_index` (re-emit, then best-effort reload — a
failure is a loud `tracing::warn`, never a dev-loop failure), and
`build_search_endpoint` returns the endpoint AND the hot handle, which
rides `StartedServer` (`cmd_serve` never swaps — production reindexes
by building and starting anew).

**Status.** The deferral is closed server-side. The remaining F09 work
is unchanged: the client matcher half (`packages/oxide/src/search.ts`)
and the TS-side parity suite.

**Deltas.**

1. **The reload is a snapshot swap, not `IndexReader::reload`.** The
   deferral's sketch assumed an in-place reader reload; the actual
   emitter wipes `dist/search/index` before every rebuild (the engine's
   own contract: a stale index is never committed onto), which unlinks
   the mmap'd segments out from under any reader. A byte-fingerprint
   swap keeps the zero-downtime property without fighting the emitter:
   readers serve the old snapshot during the rebuild and the swap is one
   pointer write under a lock.
2. **The L1 cache keys gained a generation partition.** The spec's
   "results cached in L1 for 60 s keyed by the normalized query" predates
   any reload path; with one, a keyless cache would serve pre-reload
   hits for up to 60 s after a reindex. The generation rides the key
   instead (the spec's key text is unchanged for the fixed-corpus
   engines — `generation()` defaults to 0).
3. **A configured-but-missing index stays the not-configured 404 until
   the next dev restart.** The boot emission makes this the rare path
   (the index exists before the serve side opens it); the endpoint
   cannot be attached to an already-started pipeline, so a corrupt
   dist at boot still means a restart, documented rather than hidden.
4. **Windows dev re-emission caveat.** The emitter's
   `remove_dir_all(index_dir)` runs while the same process holds mmap'd
   segment files; Linux and macOS (the dev-loop platforms) unlink open
   files and keep the mapping alive, Windows fails the delete. Production
   serve never re-emits, so the caveat is dev-only; a Windows dev host
   needs a restart per content change until the emitter learns
   swap-aside directories.

## V2-F11 — the delta-deploy engine (2026-11-12)

**What the WP asked.** "`oxide deploy` uploads only what changed and swaps
atomically" — a pure `plan()` over local/remote hash maps, immutable assets
never deleted during the skew window, uploads under `releases/<build-id>/`,
the `current` pointer written last, prune after `deploy.skewWindow`, and the
`<5%` acceptance for a one-line change (`feat/delta-deploy-engine`).

**What lands.** The engine (`crates/adapters/src/delta.rs`, the crate the
spec's sketch names): the object-safe `DeployStore` seam — load/store the
upload ledger (`RemoteManifest`), object put/delete, the atomic `current`
pointer read/write — so the `dir:` target and the future `s3://`/`r2://`
targets differ exactly there and the whole engine tests against in-memory
fakes (including the injected-failure pin); the spec-verbatim pure `plan`
(`upload`/`keep`/`delete_later`/`bytes_up`, sorted, over `scan_dist`'s
blake3-hashed `FileEntry` map — no I/O, no clock); `DirStore`, the `dir:`
target over `std::fs` with the temp+rename pointer swap, traversal-guarded
object keys and the fail-closed `DEP-MANIFEST`/`DEP-POINTER` refusals;
`deploy_dir` executing the §V2-F11 order of operations (changed immutable
chunks at their rel path first, every non-hashed file mirrored under
`releases/<build-id>/`, the ledger rewrite, the pointer LAST); `prune`
(marks expire after the skew window, the pointed release never prunes,
earliest mark wins) and `rollback` (the pointer swap to
`previous_build_id`). `deploy_dir` takes the build id and the clock as
parameters (`DeployOptions`), so callers inject both — the A04 build id and
a wall clock at the CLI layer. The `DEP-BUILDID`/`DEP-IO`/`DEP-MANIFEST`/
`DEP-POINTER`/`DEP-ROOT`/`DEP-ROLLBACK` registry rows, 15 TDD pins + 3
in-module tests, the `delta` criterion group and the `delta_deploy`
walkthrough example ride along.

**Status.** The engine is complete and green end-to-end (tests, bench,
example). The remaining F11 halves are the `s3://`/`r2://` targets (the
same `DeployStore` seam over `object_store` — the A10 `s3` feature shape
the cache crate already uses) and the `oxide deploy` CLI command (the
V8-linked CLI is outside this sandbox's compile budget; the command
wraps `scan_dist` + `deploy_dir` + `prune` + `rollback` directly — no new
engine work is gated on it).

**Deltas.**

1. **The ledger maps rel paths, not store paths.** The spec sketch's
   remote map is rel → hash; non-hashed files live under
   `releases/<build-id>/<rel>`, so the ledger keys stay stable across
   builds and the release prefix is a pure execution detail (the executor
   mirrors every non-hashed file — changed or not — into the fresh prefix,
   which is what makes a release self-contained for the tabs reading the
   new pointer; the plan's `keep` is the hash-delta view, exactly the
   sketch's).
2. **Pending deletions keep their EARLIEST mark.** A file superseded by
   several deploys in an hour must not outlive its window by being
   re-stamped; re-marks only ever lower the timestamp. The window runs
   from the first supersession, not the last deploy.
3. **`prune` reads the pointer and protects the release it names.** After
   a rollback the pointer names an old release whose copies were marked
   pending by the deploy that superseded it; a naive window prune would
   delete the rollback target out from under the rollback. The
   rollback-safety rule: a mark under `releases/<pointer-id>/` never
   deletes.
4. **The skew window is a parameter, not a config key (yet).**
   `deploy.skewWindow` is F10's config surface; until that key lands the
   window travels as `PruneOptions::window` (`DEFAULT_SKEW_WINDOW` = the
   F10 default, 15 minutes). Adding the config key later is a wiring
   change at the CLI layer, not an engine change — and it dodges the E11
   config-reference regeneration this pass cannot run (the generator
   lives in the V8-linked CLI).
5. **The manifest schema is versioned and fail-closed.** `version != 1`
   is a `DEP-MANIFEST` refusal, never a best-effort parse — a future
   engine version must not silently misread an older ledger.

**To unclose.** The `object_store`-backed `s3://`/`r2://` `DeployStore`
impl (behind the adapters `s3` feature, the cache crate's feature shape)
and the `oxide deploy` CLI command (`scan_dist` + `deploy_dir` +
`prune`/`rollback` over the parsed target spelling) — both wire-only,
no engine work gated on them.

## V2-F10 — the skew-protection pool substrate (2026-11-12)

**What the WP asked.** "After a deploy, tabs still on the old build keep
working for a window, and rollback is instant" — routing by
`x-oxide-build`, `deploy.skewWindow` (default 15m), `GenerationPools`,
rollback via `.oxide/generations/<id>` (the deploy side of the rollback is
F11's pointer swap, landed in the same pass — see the V2-F11 record).

**What lands.** The pool substrate (`crates/dispatch/src/skew.rs`, the
V8-free crate the A04 substrate lives in): the `AcceptedGenerations` trait
— the `accepts(id, now) -> Option<Gen>` contract the data-plane stage
consumes, implemented by the existing 1-deep `GenerationSet` AND by the
bounded `GenerationPool`, blanket-implemented for `&T` so a boot-chosen
source can be held as `&dyn` — and `stage_check`, the §V2-A04 stage
generic over the seam. The existing `stage_build_check` is the 1-deep
specialization delegating to it: no public signature changed, the A04
pins hold unchanged, and `tests/skew_tdd.rs::the_stage_drives_set_and_
pool_with_identical_verdicts` pins the set/pool verdict parity.
`GenerationPool` semantics: each demotion carries its OWN demotion
instant (the window runs from the demotion — two deploys ten minutes
apart expire ten minutes apart); promotes trim the expired demotions (a
deploy doubles as the housekeeping pass); the OLDEST demotion evicts
first when the bounded pool is full (the A04 "bounded by config, not
deploy frequency" rule generalized from depth 1 to depth N); re-promoting
a demoted id is the rollback path and moves it back to current;
promoting the current id is a no-op. 9 TDD pins, the `skew` criterion
group and the `skew_pool` walkthrough example ride along.

**Status.** The substrate is complete and green end-to-end. The
remaining F10 halves are the `deploy.skewWindow` config key (the E11
config-reference regeneration it requires needs the V8-linked generator),
the CLI adoption (constructing the pool at serve boot, promoting it when
`cmd_dev` reloads a build — the same A01-phase-3 wiring `GenerationSet`
itself is deferred on) and the two-generation serving pools (`GenerationPools`
serving the old build's HTML while the new one takes traffic — needs the
serve-side artifact promotion F11's `releases/` layout already lays out).

**Deltas.**

1. **`accepts` is a pure lookup; reclamation is explicit.** The trait's
   `accepts(&self)` matches `GenerationSet`'s shape (the stage holds the
   source by `&`), so expired demotions are refused on read but reclaimed
   only by `expire(&mut self)` or the next promote — a read never mutates,
   which keeps the stage lock-free.
2. **The window is per-demotion, not global.** The spec's
   `deploy.skewWindow` text reads as one window for all builds; per-
   demotion clocks are the same thing stated per build (each window
   starts at its own demotion) and is what makes "deploys inside the
   window keep their clients" true for the SECOND deploy too — a shared
   clock would expire N-2 early.
3. **Depth is a constructor parameter, not config (yet).** `capacity`
   travels with `window` in `GenerationPool::new`; both come from the
   same (deferred) config key set at the CLI layer. No engine work is
   gated on the config surface.

**To unclose.** The config key + CLI adoption (the A01-phase-3 wiring
`GenerationSet` is already deferred on) and the two-generation serving
pools.

## V2-F11 tail — the object-store deploy target (2026-11-12)

**What the deferral asked.** The parent F11 record named the remaining
halves: "the `s3://`/`r2://` targets (the same `DeployStore` seam over
`object_store` — the A10 `s3` feature shape the cache crate already uses)
and the `oxide deploy` CLI command". This pass closes the target half
(`feat/object-store-deploy-target`).

**What lands.** `crates/adapters/src/objstore.rs` — `ObjectStoreStore`,
the second `DeployStore` impl, behind the crate's new `s3` feature (the
cache crate's feature shape): `deploy_target_from_url` parses the target
spellings — `s3://bucket/prefix` (credentials/region from the standard
`AWS_*` environment, the cache handler's `from_env` convention),
`r2://bucket/prefix?endpoint=…` (the S3 API against an endpoint override;
an `r2` target without its endpoint refuses `DEP-URL`), `file://` and
`mem://` (the network-free backends the pins, the bench and the
walkthrough run over); the optional `endpoint=` query parameter also
overrides the endpoint for self-hosted `s3://`. The ledger, the objects
and the pointer ride the `object_store::ObjectStore` trait through a
dedicated current-thread tokio runtime the store owns (the trait is sync;
the engine and its pins are). The pointer swap is the object-store
atomicity contract: create-if-absent (`PutMode::Create`) on a fresh
store, then update-against-the-version-this-deploy-read
(`PutMode::Update`) — a swap that loses the race refuses `DEP-POINTER`
and never flips `current` (`swap_pointer`/`pointer_version` expose the
conditional primitive the lost-race pin drives). Corrupt state refuses
with the `dir:` target's codes through the now-shared
`parse_manifest`/`parse_pointer`/`validate_object_path` helpers
(`DirStore` delegates to the same parsers, byte-identical messages, the
15 parent pins hold unchanged). The `DEP-URL` registry row rides along
(`docs/errors.toml`, the errors reference page regenerated), 10 TDD pins
(`tests/object_store_tdd.rs`: URL parsing incl. the `r2` endpoint
refusal, ledger/pointer round trips, the conditional swap, the full
deploy layout through `list_keys`, the injected-failure pointer pin, the
idempotence + <5% acceptance, prune/rollback parity, traversal
refusals), 2 in-module parser tests, the `object_store` criterion group
(first-full vs idempotent re-deploy vs young prune over the in-memory
backend, the `delta` group's shapes for comparison) and the
`object_store_deploy` walkthrough example (network-free `mem://` by
default; a real `s3://`/`r2://`/`file://` URL as the first argument walks
the identical steps).

**Status.** Green end-to-end: `cargo test -p oxide-fw-adapters --features s3`
98 passed across the crate's targets, the bench group runs, the example
runs. The delta-deploy engine itself is untouched — the new target is a
consumer of the existing trait, exactly as the parent record sketched.

**Deltas.**

1. **The pointer swap is conditional-put, not rename.** `DirStore`'s
   temp+rename is atomic within one filesystem; object stores have no
   rename. The honest equivalent is the conditional put (create-if-absent,
   update-against-the-read-version); a lost race refuses `DEP-POINTER`
   instead of overwriting the concurrent deploy's pointer. The cache
   crate's B-11 record documents the same platform reality from the
   read side ("object stores have no compare-and-swap") — 0.14's
   `PutMode::Update` provides the write-side primitive on the backends
   that support etags, and the pin holds it for `InMemory`.
2. **`r2://` rides the S3 API with a mandatory endpoint query
   parameter** — R2 exposes the S3 API per-account endpoint; the
   credentials still come from the standard `AWS_*` environment. An
   `r2://` target without an endpoint refuses at parse time (`DEP-URL`,
   the fix names the parameter), never at first use.
3. **The bridge runtime is owned by the store.** `object_store` is async
   and `DeployStore` is sync; `ObjectStoreStore` builds a current-thread
   tokio runtime in its constructor and `block_on`s each operation.
   Calling from inside an async runtime would panic (`block_on` forbids
   it) — documented on the struct; the engine's callers are synchronous
   (the CLI, the tests).
4. **`mem://` is a first-class scheme.** The in-memory backend is not a
   test-only shorthand but a parsed target (`mem://label`), because the
   walkthrough, the bench and the CI pins need a network-free deploy
   target with the exact `ObjectStore` semantics (etags, conditional
   puts, `NotFound` shapes) the remote backends expose.

**To unclose.** The `oxide deploy` CLI command (the V8-linked CLI
remains outside this sandbox's compile budget; the command is a thin
`deploy_target_from_url` + `deploy_dir`/`prune`/`rollback` wrapper — no
engine work is gated on it), and `r2`'s account-id derivation from an
environment variable once a real R2 deployment exercises the spelling.

## V2-B08 phase 2 — the streaming nonce splicer (2026-11-12)

**What the WP asked.** B08's phase 2: "a chunk-safe splicer for the live
path, so neither the L0 fill nor the response path needs a full-document
copy", with the spec carrying the `NonceSplicer` sketch and its test set
verbatim (`crates/render/src/splice.rs` named in the spec). This pass
lands the substrate (`feat/object-store-deploy-target`, riding the same
session as the F11 object-store target).

**What lands.** `crates/render/src/splice.rs` — the spec-verbatim
`NonceSplicer`: `new(from, to)` owns an owned
`memchr::memmem::Finder<'static>` plus the needle bytes; `push(chunk,
&mut out)` splices every complete occurrence and holds back the last
`from.len() - 1` bytes so a needle split across two chunks is still
found; `finish` flushes the carry. The spec's three pins
(`splits_at_every_position` — every possible cut, `no_match_passthrough`,
`byte_at_a_time`) land verbatim, plus the carry-contract pins the live
tee needs (empty input, finish idempotence, adjacent matches with no
gaps, a needle longer than the chunks around its boundary). The `splice`
criterion group benches 20 KB / 200 KB / 2 MB bodies fed in 64 KiB
chunks against the whole-document `str::replace` baseline (8.1/80.5/826
µs replace vs 9.2/90.3/1146 µs spliced — the splicer trades ~10–40% wall
time for never holding a second copy of the document, the B08 point).
`memchr = "2.8.3"` is pinned in `[workspace.dependencies]` (already in
`Cargo.lock` transitively) and the `nonce_splice` walkthrough example
runs the fill → serve round trip across deliberately mid-nonce chunk
boundaries.

**Status.** The substrate is green (tests, bench, example). The CLI
adoption — phase 1's render-with-the-sentinel prerender and the three
`html.replace(&nonce, …)` deletions, plus the dhat <1.1× peak-allocation
acceptance — remains the A01-phase-3 slice on a V8-capable host, exactly
as the Wave-B deferral record scheduled; the fill/serve sides call the
splicer where the `.replace` calls sit today.

**Deltas.**

1. **The splicer owns its needle.** `Finder<'static>` + `from: Vec<u8>`
   makes the splicer `Send` and self-contained (the tee task can hold
   it); the spec sketch's `Finder<'static>` is produced via
   `Finder::new(&f).into_owned()`.
2. **`memchr` enters `[workspace.dependencies]`** at the lock-resolved
   `2.8.3` — the spec anticipated this ("add memchr to
   `[workspace.dependencies]` if absent; it is already in Cargo.lock").
3. **The wall-time tradeoff is measured, not hidden.** At the benched
   sizes the splicer is ~10–40% slower than `str::replace` — the
   justification is the allocation profile (O(chunk) transient vs a
   full second copy per document), which is what the B08 acceptance
   gates on (dhat `max_bytes` at the CLI layer).

## V2-F10 tail — the two-generation serving pools substrate (2026-11-13)

**What the deferral asked.** The parent F10 record named the remaining
halves: "the `deploy.skewWindow` config key, the CLI adoption (constructing
the pool at serve boot, promoting it when `cmd_dev` reloads a build) and the
two-generation serving pools (`GenerationPools` serving the old build's HTML
while the new one takes traffic — needs the serve-side artifact promotion
F11's `releases/` layout already lays out)". This pass closes the pools
half (`feat/generation-serving-pools`).

**What lands.** `crates/dispatch/src/pools.rs` — `GenerationPools<P>`, the
§V2-F10 design step 1 structure holding a serving HANDLE per generation
(the generic `P` is the caller's; the spec instantiates it over
`Arc<dyn Pool>`, this V8-free crate stays pool-agnostic). `route(id, now)`
is the step-2 routing decision in one lookup: the absent id (every HTML
navigation and header-less data-plane request) and the current id take the
CURRENT pool, an accepted demoted id takes THAT generation's own handle
(`previous_pool_serves_old_build_header` — the old build's data plane keeps
serving old tabs), an unknown or expired id routes `None` and the §V2-A04
409 authority stays [`stage_check`], which now drives the pools through the
existing `AcceptedGenerations` seam (`accepts` is `route`'s generation
half, so the set/pool/pools verdict parity is pinned by
`tests/pools_tdd.rs::the_stage_drives_set_and_pools_with_identical_
verdicts` — the three id substrates agree on every §V2-A04 verdict).
Lifecycle: `promote(id, pool, now)` demotes the current id WITH its live
handle (per-demotion windows, expired trimmed first, oldest evicted at
capacity — the A04 bounded-by-config rule generalized from ids to handles),
the re-promote of a demoted id installs the caller's fresh handle and drops
the stale entry (the rollback path), `expire(now)` drops expired HANDLES
without unlinking anything ("at window end it drops the previous pool and
unlinks nothing" — client chunks are content-hashed), and
`evict_oldest()` is the brownout early-drop (the F12 hook) returning the
evicted handle for a graceful drain-and-shutdown. 9 TDD pins
(`tests/pools_tdd.rs`), the `pools` criterion group (the absent-id fast
path, the current hot path, the depth-8 oldest-demoted walk, the miss and
the stage-through-the-seam shape) and the `serving_pools` walkthrough
example ride along.

**Status.** Green end-to-end: `cargo test -p oxide-fw-dispatch` 128 passed
(119 parent + 9 new), the bench group runs, the example runs. The id-only
`GenerationPool` and the `stage_check` signatures are untouched — the new
structure is a third implementor of the same seam, exactly as the parent
record sketched.

**Deltas.**

1. **Capacity N, not literally two.** The spec's sketch bounds memory "at
   two generations" (`previous: Option<...>`); the landed shape is the
   bounded deque the id-only pool already generalized (capacity 1 IS the
   sketch's two-generation shape, 0 keeps only the current build). The
   config key that will size it (`deploy.skewWindow`, deferred) sizes
   depth the same way it sizes the window.
2. **The handle is the caller's.** `P` is generic instead of a
   `dyn Pool` trait defined here: the V8-free crate must not know the
   server's worker-pool type, and the spec's `Arc<dyn Pool>` spelling
   drops straight in at the CLI layer (the same seam choice the
   `AcceptedGenerations` trait made — the boot-time-chosen source stays
   outside the substrate).
3. **The no-op promote keeps the LIVE handle.** Promoting the current id
   ignores the caller's (fresh) handle: an idempotent re-deploy must not
   swap a serving pool for a newly-booted identical one mid-flight. The
   rollback path is the explicit re-promote of a DEMOTED id, which
   installs the fresh handle and drops the stale entry (pinned).

**To unclose.** The `deploy.skewWindow` config key (the E11
config-reference regeneration it requires needs the V8-linked generator),
the CLI adoption (booting the pools from `dist/` + F11's `releases/<build-id>/`
layout, promoting on `cmd_dev` reload, the 30 s expiry task and the F12
brownout trigger calling `evict_oldest`) and the E2E `tests/skew.sh` — all
A01-phase-3 slices on a V8-capable host.

## V2-C01 — the native-compiler seam (2026-11-13)

**What landed.** `crates/compiler/src/native/mod.rs` — the §V2-C01
substrate, V8-free and complete per the WP's own acceptance:

- the `NativeCompiler` trait (string in, string out — no AST type crosses
  the boundary, each upstream pins its own `oxc`), the `Backend` selection
  copy, `NativeRegistry` (`pick` with the once-only `OnceLock` health
  fixture, `register` as the injection seam, `report` for `oxide doctor`)
  and `cas_tool_version` (`native:{id}@{version}`, the D40 CAS key
  dimension);
- the config surface: `framework.compilerBackend` typed (`CompilerBackend`:
  `auto` default | `native` | `node`), schema'd (the E11 DOC-001 gate),
  load-time-validated (`CFG-BACKEND` names the three modes before the
  typed parse) and env-overridable (`OXIDE_COMPILER_BACKEND`);
- the crate adopted the workspace lints (the pedantic hygiene pass — the
  or-patterns/`# Errors`/`must_use`/cast fixes across
  `oxc_backend.rs`/`lib.rs`/`framework.rs`/`lexer.rs`/`cjs_exports.rs`/
  `generate_params.rs`, the 236-line `lex` split into scanner helpers,
  the `ast_server_fn_stubs` closure extraction) and shed two dead
  dependencies (`oxc` umbrella — cargo's unused-dependencies lint — and
  `regex`, both unreferenced);
- 10 TDD pins (`tests/native_registry_tdd.rs`, driven by an injected
  `FakeNative` — the spec's own test plan for the pre-C02 tree), the
  config suite (`tests/compiler_backend_config.rs`), the `native` criterion
  group and the `native_registry` walkthrough example.

**Spec deltas (recorded, not silent).**

1. **The config key is a typed enum, not the sketch's `Option<String>`.**
   The config crate holds every key typed (`deny_unknown_fields` +
   load-time validators); a bare string would push validation into every
   consumer. The engine-side `native::Backend` stays a separate copy per
   the recorded F04 ruling (config → engine conversion is wiring, not a
   dependency).
2. **`report()` returns a `NativeReport` struct, not the sketch's tuple.**
   The doctor prints `id version healthy`; a named row is self-documenting
   and the field order stays stable in the face of additions.
3. **`OXIDE_COMPILER_BACKEND` needs an alias table.** The env layer
   derives dotted paths by lowercasing + underscore-splitting, so a
   camelCase config key is unreachable (`compiler.backend` ≠
   `framework.compilerBackend`). `ENV_DOTTED_ALIASES` maps the derived
   spelling onto the real path; it is the first camelCase key to need it
   and later keys extend the table.
4. **`from_enabled_features()` is empty today.** No native backend is
   compiled in — C02–C04 land the real backends behind their vendored
   upstreams (D27) and push into the registry; the feature flags are NOT
   pre-declared (a feature with no backend behind it is dead scaffolding).
   `auto` answers the Node pool for every framework and `native` refuses
   `CMP-NAT-001` until then — the C01 contract the spec's own
   `registry_pick_node_always_none` pin holds.

**To unclose.** The CLI adoption — `compile_one` consulting `pick` ahead
of the tooling pool, `oxide doctor` printing `report`'s rows,
`PrecompileKey::tool_version` folding `cas_tool_version`, and the
`registry_auto_falls_back_when_unhealthy` acceptance at the CLI layer —
is an A01-phase-3 slice on a V8-capable host, exactly like the A02/A05/A06
substrate adoptions.

## V2-C09 — the image job model (2026-11-13)

**What landed.** `crates/assets/src/image_jobs.rs` — the §V2-C09
substrate: the bounded CPU lane (`available_parallelism() - 1`,
`with_lanes` for the config-sized shape), the smaller AVIF lane, per-key
single-flight over a `DashMap` of shared futures (losers join the leader's
outcome; failed/panicked jobs drop their entry so the next request
retries), the injectable `ImageCas` hooks (lookup before work, store after
success), `spawn_blocking` placement and the `AST-JOBS` panic path; the
`images.avifSpeed` config key (1..=10, default 6) threads into ravif
through `encode_avif_at_speed`; `OxideError` gained `Clone` (additive —
the shared outcome's error half). 7 TDD pins + the `image_jobs` criterion
group + the `image_jobs` walkthrough example.

**Spec deltas (recorded, not silent).**

1. **The single-flight key is the module's `cache_key()` `String`, not the
   cache crate's `KeyStr`.** `oxide-fw-assets` does not depend on
   `oxide-fw-cache` today; importing the heap-free key type would pull the
   redb/odf tree into every assets build for a type the module immediately
   re-hexes. The CLI adoption can switch the key spelling when it wires
   the real CAS — the seam (the `ImageCas` trait) does not change.
2. **`OxideError` is `Clone` now.** The single-flight shares ONE awaited
   outcome across N waiters, so the error half must clone. The graph crate
   already worked around the missing derive (`OxideError(e.0.clone())`);
   the derive makes that workaround unnecessary.
3. **The CAS is an injected seam, not a concrete tier.** The spec's step c
   ("CAS lookup before work, CAS store after") is the `ImageCas` trait;
   the endpoint feeds the real tiered store at the CLI adoption. Tests
   pin the contract against fakes (the spec's own test plan).

**To unclose.** The endpoint wiring — constructing `ImageJobs` from
`images.maxConcurrency`/`images.avifSpeed`, feeding the tiered CAS, and
the `oha -c 64` acceptance run against a live server — is an A01-phase-3
slice on a V8-capable host. The assets crate's own pedantic adoption (the
crate is not yet in `[lints] workspace = true`) is the next crate-owned
hygiene pass; the new module is held to the same standard by hand.

## V2-C07 — the SVG optimizer seam (2026-11-13)

**What landed.** `crates/assets/src/svg.rs` — the §V2-C07 substrate: the
`SvgOptimizer` trait (string in, string out — no DOM type crosses the
boundary, the C01 shape; `id()`/`version()` are the D40 key dimensions),
`IdentitySvg` (the always-available byte-identical passthrough),
`OxvgSvg` behind the crate's `svg-oxvg` feature (`oxvg_optimiser` 0.0.9
with `oxvg_ast` 0.0.9, pinned in `[workspace.dependencies]`; spike-passed
per the C-series rule — the crate builds and the safe preset preserves
the WP's three named fixtures), the preservation rules enforced on every
optimizer output by `assert_preserved` (the source's `viewBox` value
verbatim, every referenced id from `<use href>`/`xlink:href`,
`url(#…)` and `aria-labelledby`/`aria-describedby`, and the output
parsing back as well-formed XML through `roxmltree` — a violation is
`AST-SVG-001`, the source kept), `AST-SVG-003` for a backend's own
parse/jobs failure and `AST-SVG-002` refusing `svg.optimize: true`
without the feature (the `CMP-NAT-001` shape); the
`svg:{id}@{version}:{blake3-16hex}` CAS key; the `svg.optimize` config
surface (typed `SvgConfig`, schema'd per the DOC-001 gate, default off,
the `CFG-UNKNOWN-KEY` top-level allowlist updated, the checked-in
`schemas/oxide-fw-config.schema.json` regenerated); `optimize_for_build`
as the single call site the import pipeline uses (knob-off is
byte-identical; every refusal warns its diagnostic code and keeps the
source, so a misbehaving optimizer degrades to "unoptimized" instead of
breaking the build); the cli `svg-oxvg` forwarding feature and the
`.svg?react` wire. 16 TDD pins (the rules driven through FAKES so they
hold against any `SvgOptimizer`, the three named fixtures plus the ≥15%
total-shrink acceptance through the real one), the `svg_optimizer`
criterion group and the `svg_optimizer` walkthrough example ride along.

**Spec deltas (recorded, not silent).**

1. **The config key is `svg.optimize`, not `assets.svg.optimize`.** The
   spec writes `assets.svg: { optimize: true }`, but the tree's config
   groups are top-level pipeline concerns (`images.*`, `fonts.*`,
   `og.*`); there is no `assets` group. `svg` lands as a top-level group
   with the same shape. Renaming later is a one-line schema move.
2. **The rules are enforced on the wrapper level, not trusted to the
   optimizer.** `assert_preserved` takes `(source, optimized)` and is
   public — the pins drive it through rule-breaking fakes, so the
   contract holds for a third-party `SvgOptimizer` too (the spec's
   "rules the wrapper enforces" reading).
3. **`roxmltree` rides every assets build (non-optional); the OXVG pair
   is feature-gated.** The well-formedness parse-back is a rule, and the
   rules must hold even when `svg-oxvg` is not compiled in (an injected
   backend can still produce garbage). `roxmltree` 0.21.2 is the parser
   `oxvg_ast` 0.0.9 itself uses, so no new version dimension enters the
   lockfile.
4. **The bundled backend runs `Jobs::safe()`, not `Jobs::default()`.**
   The correctness preset is documented by OXVG as "less likely to
   visually change the document" — the only honest default for a build
   pipeline whose wrapper refuses rule-breaking output. The ≥15%
   shrink acceptance holds under it on the named fixtures.

**To unclose.** The `?url` / `?raw` / `?inline` import variants the WP's
hook step lists do not exist in the tree (only `.svg?react` does — the
crate's §6.7 asset-query model parses the spellings, but no pipeline
stage consumes them); wiring them is the A01-phase-3 import-pipeline
slice, at which point each site calls `optimize_for_build` the way the
`.svg?react` site already does. The assets crate is still not in
`[lints] workspace = true` (the C09 note's next crate-owned hygiene
pass); the new module is held to that standard by hand.

## V2-E02 — the typed-search-params typegen substrate (2026-11-13)

**What landed.** `crates/router/src/typegen.rs` — the §V2-E02 Rust
substrate: the public `declares_search_schema` scanner (token-safe: a
`export const searchSchema` with an initializer or a type annotation
marks the route; a `function` export is not a Standard Schema value; a
longer binding like `searchSchemaV2` is a different identifier), and
typegen's **delegated inference** emission — the WP's core design (the
generator cannot infer a Standard Schema's output, so the emitted
`routes.d.ts` imports the value and references
`S.InferOutput<typeof searchSchema>`, letting `tsc` do the inference at
check time): the `StandardSchemaV1 as S` header import, the per-route
`import type { searchSchema as s_<sanitized-pattern> }` from an
extensionless relative path resolved from `.oxide/types`, the
`LoaderEvent.search` dimension (the raw `searchParams` record stays on
every route — the record is what the request hands over, the typed
`search` is what the schema validated), the `RouteSearch` map + the
`Search<R>` module augmentation (the tree's `RouteParams`/`RouteData`
pattern standing in for the sketch's single `RouteMap`), and the
per-route `href` opts overload. `routes.json` carries `hasSearchSchema`
from the same scan. 8 TDD pins, the `typegen_search` criterion group and
the `typed_search` walkthrough example ride along.

**Spec deltas (recorded, not silent).**

1. **No single `RouteMap` — the tree's named maps.** The sketch emits
   one `RouteMap` interface; the tree's typegen already ships
   `OxideRoutes`/`RouteParams`/`RouteData`/`LoaderEvent` maps and a
   module augmentation, and the pins hold those shapes. The search
   dimension lands as `RouteSearch` + `Search<R>` in the same idiom.
2. **The typed `search` field is additive.** `LoaderEvent` keeps
   `searchParams` on every route and gains `search` only where a schema
   is declared — the delegation imports, the map and the augmentation
   only emit when at least one route declares, so a schema-less
   project's generated bytes are unchanged.
3. **`href` search options are an additional overload.** The sketch
   folds params + opts into one signature; the tree already emits
   per-route overloads (including the i18n locale-last form). The opts
   overload is added alongside — existing call sites (including
   `href(route, params, locale)`) still compile.
4. **The scan requires a routes root.** `typegen` (no root) reads no
   server modules, so the delegation activates only under
   `typegen_with_root` / `typegen_with_root_and_i18n` — the CLI build
   path, which is the only caller that has the root.

**To unclose.** The client half — the per-adapter `Link`/`goto` generic
types (`packages/react|vue|svelte|solid`), the runtime `parseSearch`
(`schema['~standard'].validate`, issues as a 400 page / redirect-to-
corrected) and the F16 `SearchParamRead` interplay (a declared schema
satisfies the why-dynamic search-param read) — is the packages-side
slice; the Rust substrate above is its contract.

## V2-C08 — the glyph-usage font-subsetting seam (2026-11-13)

**What landed.** `crates/assets/src/font_subset.rs` — the §V2-C08
remaining half on top of the already-landed helpers: the spec-verbatim
`FontSubsetter` trait (font bytes + a harvested `BTreeSet<char>` in,
subset bytes out; object-safe, plus the D40 `id()`/`version()` key
dimensions the C07 seam carries), the bundled `SkrifaSubsetter` behind
the crate's `subset` feature, `select_subsetter` (feature off refuses
`AST-FNT-002` — the `AST-SVG-002` shape, naming the flag and the
fallback), the `font:{id}@{version}:{blake3-16hex}` CAS key and
`subset_used_for_build` as the one definition site the bundler calls
(always-needed set unioned into every request; every refusal
warn-degrades to the verbatim bytes). The backend's honesty half, found
by the spec's own cmap pin: `subsetter`'s `Profile::pdf` copies the
original cmap VERBATIM (only glyf/loca/cff/head/hmtx/post are processed),
so a dropped glyph keeps its cmap entry and renders as a blank instead of
falling back to the next face — the bundled backend therefore rewrites
the subset's cmap (`cmap_rewrite`: a fresh format-12 subtable mapping
exactly the kept codepoint→glyph pairs, the sfnt re-serialized with
per-table checksums and the recomputed `head.checkSumAdjustment`; any
rewrite refusal ships the raw subsetter output, so the seam only ever
improves). The post-prerender harvest the
spec names: `harvest_html_codepoints` (the `lol_html` `text!`-over-body
walk, `<script>`/`<style>` subtrees removed before the text handler runs
so a minified bundle's identifier soup never grows the shipped font) and
`harvest_text_codepoints` for the content-collection field text. The CLI
wire-up: `emit_fonts` harvests every `*.html` under `dist` plus every
configured collection (entry bodies + frontmatter strings) into
`FontSpec.used_chars` when `fonts.subset` carries `used`. 15 TDD pins
including the spec's skrifa cmap pin over the committed
`tests/fixtures/subset-fixture.ttf`; the `font_subsetter` criterion
group and the `font_subsetter` walkthrough example ride along.

**Spec deltas (recorded, not silent).**

1. **The `AST-FNT-001` raise did not need re-raising.** The C08 status
   row listed "the raised `AST-FNT-001` refusal" as remaining, but the
   refusal has been live since the V2-R02 config-key half
   (`check_config`'s target-independent fonts arm, pinned in
   `crates/adapters/tests/config_refusal_tdd.rs` and consulted by the
   CLI build) — the row was stale at "remain" and is corrected to
   `merged`.
2. **The static-list harvest is retired, not extended.** `bundle` used
   to subset to the glyphs of the literal strategy strings (configuring
   `subset: ["latin"]` shipped a font containing only `l`,`a`,`t`,`i`,
   `n`). In the landed reading, `latin`/`unicode-range` are range
   declarations, not glyph-usage sources: only the `used` spelling
   subsets (through the seam, with the harvest); every other spelling
   ships the source byte-identical, pinned by the TDD suite.
3. **`fontcull` stays a stand-in.** The deps report's `fontcull`
   (fontations/klippa) is not published on crates.io; the WOFF2-row
   deviation shape applies. The `FontSubsetter` seam is the injection
   point a published `fontcull` drops into — `SkrifaSubsetter` (the
   `subsetter` crate driven by `skrifa`'s charmap) ships as the bundled
   backend behind `subset`.
4. **`lol_html` rides every assets build (non-optional).** The harvest
   is a build-side step (V2.0 item 3: the pure walk belongs where tests
   are cheap), not a subsetting-engine detail — only `subset_core` and
   its callers are feature-gated, mirroring the C07 `roxmltree` ruling.
5. **One char set for every family** (the spec's honest simplification,
   verbatim): per-element `font-family` resolution stays out of scope.

**To unclose.** The §16.2 emit path keeps its TrueType container (the
16-B WOFF2 deviation, tripwired); the acceptance's ≥60% shrink number
belongs to a real docs fixture build through the CLI (the committed
fixture shrinks 90%+ on a two-letter request — the bench pins the
per-font cost).

## V2-B07 — the parallel-precompile engine (2026-11-13)

**What landed.** `crates/compiler/src/precompile_engine.rs` — the §V2-B07
phase-2 engine: `PrecompileJob` (the discovered job: the precompiled-map
path, the tooling kind, the source), `compile_jobs` (the bounded
`buffer_unordered` window generic over the injected per-job pass; every
job produces exactly one row, folded back to INPUT order so the caller's
map fold is deterministic under out-of-order completion; a failing job
rides its own row and never cancels its siblings) and `width_for` (the
spec's `workers * 2` sizing as one definition site, floored at 1). The
CLI's `precompile_entries` (`crates/cli/src/precompile.rs`) keeps its
phase-1 discovery walk (the existing `scan_import_specs`/
`resolve_relative` helpers unchanged, BUG-MAPKEY normalization intact)
and compiles the discovered jobs through the engine — cache lookup and
the pool call stay inside the per-job pass, exactly the spec's
`compile_one` shape. The `precompile_engine` criterion group
(fake-pool 20 jobs at width 1 vs 4, 200 instant jobs) rides along.

**Spec deltas (recorded, not silent).**

1. **The engine lives in the V8-free compiler crate, not
   `precompile.rs`.** The spec's file column says `cli/src/precompile.rs`,
   but V2.0 item 3 puts pure logic where tests do not pay the V8 link
   cost — the three named pins (determinism under out-of-order
   completion, the ≥4 overlap window, failure isolation) run against
   fakes in the compiler crate in seconds; the CLI keeps the discovery
   walk and the pool-specific `compile_one` dispatch. Recorded per the
   own-your-files rule.
2. **The window is `workers * 2` as specified; `compile_one` is the
   injected pass.** Cache handling (`cached_compile`/
   `cached_compile_optional` for `mdx`) stays per job inside the pass,
   so the CAS short-circuit semantics are unchanged per kind.
3. **The cold-fixture 4x number is deferred to the real-pool adoption.**
   The bench pins the structure (width 1 vs width 4 over fake-latency
   jobs); an honest e2e number needs the tooling pool's real compilers,
   which this environment cannot run.

**To unclose.** C01's CLI adoption (native compilers plugging in at
`compile_one` with `width = available_parallelism()`) consumes the same
engine unchanged.


## V2-E07 deltas (feat/llms-mcp-registry)

- **`replay.run` IS in the MCP registry now.** The delta below recorded it as absent while the `oxide replay` CLI command was deferred; the F15/E03 CLI delivery (see "V2-F15 — the CLI wiring" below) registered `replay.run` over `run_replay` — the tool's source is the same `ReplayRunner` one-definition the command drives, so no stub: the registry ships eight tools with real sources.
- **`llms-full.txt` reads the prerendered page text, not the content collections.** The spec's source is "route text content from content collections, Markdown", but `content.collections` carries no collection→route-URL mapping today, so a collection entry cannot be attached to its route honestly. The emission walks the prerendered pages (the sitemap's own ground truth) and extracts the visible text (script/style stripped) instead.
- **`-W pedantic` vs `-W clippy::pedantic`.** clippy 1.100 refuses the bare `pedantic` spelling under `-D warnings` (renamed-and-removed-lints); the gate runs `-W clippy::pedantic` (the same lint group).

## V2-F15 — the record-and-replay substrate (feat/dispatch-record-substrate + feat/dispatch-replay-substrate, 2026-11-14)

**What lands.** The V8-free capture and playback halves of §V2-F15, in `crates/dispatch`:

- `crates/dispatch/src/record.rs` — the versioned `RequestRecord` schema
  (camelCase, `version: 1`): the request line-set with the secret trio
  (`authorization`/`cookie`/`set-cookie`) redacted to `sha256:<12 hex>` —
  the D30 cross-implementation digest, so the TS side hashes the same
  bytes with the same truncation — the matched route id and params, the
  middleware locals, each loader input and output serialized verbatim
  (the ODF-bearing shapes), each outbound fetch op (status, redacted
  headers, body base64, capped at 1 MB with a `truncated` marker — a
  dropped op would break the ordered replay, so oversized bodies are
  kept partial instead), cache op results, the build id, and the
  `Date.now()`/`Math.random()`/`crypto.randomUUID()` observation
  sequences. The `RecordPolicy` is the spec's "dev always when
  `record: 'errors' | 'all'`, prod sampled `record: { sample: 0.001,
  onError: true }`" as a pure decision (`All` / `Errors` /
  `Sampled { sample_rate, on_error }`, 5xx counting as the error the
  `onError` override and the `'errors'` mode capture — a 4xx is a
  legitimate client outcome, not a repro bug). The `RecordSink` seam
  stores records; `DirRecordStore` writes
  `<root>/.oxide/records/<request-id>.json` atomically (temp + rename,
  the F11 pointer convention) and refuses traversal-shaped ids with
  `RPL-001`. The `RecordingLoaderRunner` is the wire-up seam: a
  `LoaderRunner` decorator the dispatch closure wraps its real runner
  in, so every load the §9.2 graph drives lands in the record without
  the runner, the graph or the loaders knowing about recording.
- `crates/dispatch/src/replay.rs` — the playback half: the
  `ReplayMatcher` plays the record's fetch ops IN ORDER (method + URL +
  request body — the spec's "body hash" identity as decoded-byte
  equality — and a mismatch or exhausted record refuses `RPL-002`
  naming both the call the replay made and the call the record
  expected; a failed take consumes nothing, so the corrected call plays
  on), the `ReplaySequences` streams the recorded non-deterministic
  values in observation order (`RPL-004` on a drained stream),
  `warn_stale_build` is the `RPL-003` warn-but-run verdict, and
  `replay_diff` is the `--diff` comparison: the nonce normalization
  blanks every `nonce="<32 hex>"` value (the per-request
  `server::nonce_for` shape), the header compare skips the re-derived
  trio (`content-security-policy` — the L0 cache already treats it as
  re-derivable state, `date`, `set-cookie`), and the
  `ReplayDiff { ok, divergences }` shape serializes camelCase for the
  `--json` surface.

The `RPL-001` registry row is added and `RPL-002`/`RPL-003` flip off
`spec_only` (they have production call sites now); `RPL-004` is new.
10 capture pins (`tests/record_tdd.rs`) + 8 playback pins
(`tests/replay_tdd.rs`), the `record`/`replay` criterion groups and the
`record_capture`/`replay_replay` walkthrough examples ride along. The
regeneration of `docs/reference/errors.md` also closed a pre-existing
drift: the page was missing the `AST-SVG-001..003` rows and the
`DEP-URL` message update, and listed `RPL-002`/`RPL-003`/`BLD-409`/
`OX-SRC-001`/`PRM-001`/`RFN-001` as landed while the registry marks
them `spec_only`.

**Deltas.**

1. **The 1 MB cap truncates rather than drops.** The spec's "body
   base64, up to 1 MB" reads as a cap; dropping the op would break the
   ordered replay contract, so oversized bodies keep their first 1 MB
   and the `truncated` markers say so — the replay consumer knows the
   bytes are partial.
2. **The sampling roll is caller-supplied.** The decision stays a pure
   function of `(mode, status, roll)`; the wiring mints one uniform
   roll per request (testable, no hidden RNG in the substrate). The
   `record:` config key that feeds it is the deferred CLI half.
3. **The diff skips the re-derived header trio.** The spec says
   "compares to the recorded response (normalizing the nonce)" and no
   more; a strict compare would flag every response — the CSP header is
   re-derived per request (the L0 cache's own convention: stored
   bodies carry no CSP header), `date` is the wall clock and
   `set-cookie` may legitimately re-mint. The nonce normalization
   covers the body half exactly as written.

**To unclose.** The worker's `globalThis.__oxide_replay` install (the
`OXIDE_REPLAY=<file>` env and the fetch/clock overrides), the
`oxide replay <id> [--diff] [--json]` CLI command, the MCP `replay.run`
tool (it joins when the CLI command exists — the E07 note above), and
the `record:` config key with its schema entry and the E11
config-reference regeneration — all A01-phase-3 slices on a V8-capable
host, same deferral shape as the `chain`/`locals`/`pools` substrates.

## V2-F15 — the replay-runner seam + the `record:` config key (feat/replay-runner-seam, 2026-11-14)

**What lands.** The V8-free completion of §V2-F15 — everything the
feature still needs that does not link V8 — in two halves:

- `crates/dispatch` — the replay-RUNNER seam, the one-definition
  orchestration the `oxide replay <id> [--diff]` command and the MCP
  `replay.run` tool drive once the CLI wiring lands:
  `crates/dispatch/src/record.rs` grows the read side of the store seam
  (`RecordSource::load` — the write side's `store_key` traversal guard
  shared, the missing record a load-shaped `RPL-001`, and the record
  module's "the replay side refuses a record whose version it does not
  know" contract now ENFORCED at the only door a record enters through:
  a foreign schema version refuses `RPL-005`, naming the version it saw
  and the version this build reads); `crates/dispatch/src/replay.rs`
  grows the `ReplayExecutor` trait (the re-execution seam — the CLI's
  real implementation drives the captured line-set, route id, params and
  locals back through the V8 pipeline; test and example hosts implement
  it over fakes, keeping the runner V8-free) and the `ReplayRunner`
  (load → the `RPL-003` stale-build warn-but-run verdict → re-execute
  → `replay_diff`), returning the `ReplayOutcome` (record + executed
  response + staleness + diff, camelCase-serialized; the executed body
  rides base64 on the wire, the record's own convention). The
  executor's and the store's refusals surface unchanged — the runner
  never wraps them.
- `crates/config` — the `record:` config key (Appendix V2.L:
  `false | 'errors' | 'all' | { sample, onError }`): the typed
  `RecordConfig` (flat enum, `Off(bool)`/`All`/`Errors`/`Sampled`), the
  raw-`Value` load validator (`CFG-REC` names `record: true` — the
  boolean spelling that exists is `false` — an unknown word, an
  out-of-range or non-number `sample`, a non-boolean `onError` and an
  unknown knob, each with the valid spellings as the fix), the
  `KNOWN_KEYS` typo-suggestion entry and the schema section (the union
  shape, so editors complete the word modes and the sampled object
  alike). The key is inert at the wiring until the CLI's record/replay
  adoption lands — the same RFN-002 shape the `search` key used — and
  its defaults are the spec's prod example verbatim
  (`{ sample: 0.001, onError: true }`, so `{}` is the spec shape).

The `RPL-005` registry row is added, `RPL-001`'s message widens to the
store's two operations ("cannot write or load"), and `CFG-REC` joins
the registry — which closed a pre-existing drift: `CFG-LLMS` had
production call sites (`validate_llms`) and NO registry row (the
coverage gate is a V8-linked CLI test, so the omission was invisible in
the constrained sandbox). 10 runner pins (`tests/replay_runner_tdd.rs`)
+ 13 config pins (`tests/record_config.rs`), the `replay_runner`
criterion group and the `replay_runner` walkthrough example ride along.
The schema regeneration also closed a second pre-existing drift: the
committed `schemas/oxide-fw-config.schema.json` was missing the whole
`llms` section (E07 landed without regenerating it), and
`docs/reference/config.md` was missing both the `llms` and — until this
milestone — the `record` sections (hand-patched in the generator's
format, to be confirmed byte-identical by `docs generate --check` on a
capable host).

**Deltas.**

1. **`RPL-001`'s message widened, not a new code for load.** The store
   failure is one code across both operations; the message now names
   both ("cannot write or load"), the FIX is per-operation (the write
   side's writability hint vs the load side's re-record hint).
2. **`record: true` parses, then refuses at load.** The untagged-union
   deserializer accepts the boolean into `Off(bool)` and the raw pass
   refuses it with `CFG-REC` naming the valid spellings — an authoring
   bug gets the house diagnostic instead of a bare serde type error.
3. **A pre-existing green-gate bug fixed on the way** (not a delta of
   this feature, recorded here because the suite exposed it):
   `validate_llms` rejected the `null` `title`/`summary` the serialized
   `OxideConfig::default()` carries — every default-config load failed
   `CFG-LLMS`, and `crates/config`'s `brownout_config` loads were
   failing on pristine HEAD. Null is now treated as absent (the
   serialized `Option::None`), and the brownout loads are green again.

**To unclose.** The worker's `globalThis.__oxide_replay` install (the
`OXIDE_REPLAY=<file>` env and the fetch/clock overrides), the
`oxide replay <id> [--diff] [--json]` CLI command and the MCP
`replay.run` tool — all three are now THIN wrappers over the landed
`ReplayRunner`/`RecordSource` seams, and remain A01-phase-3 slices on a
V8-capable host, same deferral shape as the
`chain`/`locals`/`pools` substrates. The `record:` key's wiring into
`RecordPolicy` rides with them.

## V2-F15 + E03 — the CLI wiring (feat/record-replay-cli-wire, 2026-11-15)

**What lands.** The V8-linked halves of §V2-F15 and the E03 command —
everything the previous records deferred to "the CLI adoption":

- `crates/cli/src/record_wire.rs` — the pipeline's capture: the
  `RecordCapture::from_config` gate over the landed `record:` key (off
  keeps the pipeline byte-identical), the `RecordingLoaderRunner`
  decorator and the `ResponseObserver` riding the one `finish_response`
  funnel so every finished response's headers are stamped.
- `Command::Replay` + `run_replay` — the `oxide replay <id> [--diff]
  [--json]` command as the thin wrapper over the landed `ReplayRunner`;
  the CLI's `ReplayExecutor` re-executes the captured request against a
  started server (`ReplayHttpExecutor`), so the replay is a real HTTP
  round trip, not a synthetic re-render. `replay:drain` (the MCP tool's
  streaming sibling) rides the worker `Pool::call_all` broadcast.
- The MCP `replay.run` tool registered over `run_replay` — the E07
  registry delta above is closed; the registry ships eight tools with
  real sources.
- `cmd_migrate` — the E03 command as the thin wrapper over the landed
  planner (`plan_for`/`detect_framework`/`apply_plan`); the superseded
  `crates/cli/src/migrate.rs` engine is DELETED (the planner is the one
  definition site — the old engine's path mapping could drift from the
  planner's, which is exactly the duplication the plan-is-data ruling
  forbids).

**Deltas.**

1. **Streaming captures accumulate, then decide.** A streamed response
   body is not pinned chunk-by-chunk in the record while it streams (an
   unbounded in-flight buffer): the capture accumulates the stream's
   chunks and, at stream end, applies the policy decision (size cap,
   sampling) to the whole body. A record's body is still complete
   bytes; the decision point moved past the stream's end.
2. **`server-timing` joins the re-derived-header skip list.** The diff's
   header comparisons already skip `content-security-policy`/`date`/
   `set-cookie` (re-derived per run); `server-timing` is measured per
   run too (the pipeline stamps the total duration), so a byte-diff on
   it would flag every replay as divergent. It is skipped at the same
   definition site.
3. **The worker install rides the record module.** `globalThis.
   __oxide_replay`'s env/override install lives with the worker's other
   replay plumbing (`crates/jshost/worker/oxide-worker.mjs` + the host
   install), pinned by the worker's own node tests; the CLI command and
   the MCP tool share `run_replay`.

**Verification posture.** Wired end-to-end in this tree — CI
verification pending: the `#[ignore]` e2e pin
(`replay_reproduces_a_recorded_request_without_network`, the two-phase
record-then-replay over the smoke app) and the `replay_wire` example
execution run on CI's V8-capable host; the V8-free pins (record,
replay, runner, config) ran in-session.

## V2-F10 — the `deploy:` config key (feat/config-deploy-key, 2026-11-14)

**What lands.** F10's config half — the `deploy:` section the spec's
Appendix V2.L names (`deploy?: { skewWindow?: string; generations?:
number } // F10, F11 (defaults '15m', 3)`), in `crates/config`:

- the typed `DeployConfig` (`skewWindow` the duration string, `generations`
  the F11 `releases/` retention count; `deny_unknown_fields` so a typo'd
  knob cannot silently disable skew protection), with the spec's defaults
  verbatim (`'15m'`, `3`);
- `parse_duration_secs` — the §14 literal grammar extended to time
  (`<n>s|m|h|d`, plain integers = seconds; checked arithmetic), the
  public parse the wiring converts the validated window into the pools'
  expiry duration with;
- the raw-`Value` load validator (`CFG-DEPLOY`): the window must parse
  the grammar and be physically useful (a zero window expires the
  previous generation before a single request crosses the deploy — the
  DSP-112 shape), the generation count a whole number ≥ 1, unknown
  knobs named, wrong-typed knobs refused;
- the schema section (described, defaults declared — the E11 DOC-001
  gate), the `KNOWN_KEYS` typo-suggestion entry, the regenerated
  committed schema, the `CFG-DEPLOY` registry row + reference-page row
  and the `docs/reference/config.md` section (hand-patched in the
  generator's format, `docs generate --check` to confirm on a capable
  host).

12 TDD pins (`tests/deploy_config.rs`: the spec defaults, the camelCase
parse, the grammar table, the refusal table, the typo surface, the
schema shape). The section is inert at the wiring until the F10 CLI
adoption lands — the same RFN-002 shape the `search`/`record` keys
used; the engine-side default (`DEFAULT_SKEW_WINDOW`,
`crates/adapters` `PruneOptions::window`) stays the plumbing default.

**Deltas.**

1. **The window stays a string literal, not a seconds number.** The
   spec types `skewWindow?: string` (and the F10 body names `'15m'`);
   `StreamConfig`'s `ttl_secs` precedent (a seconds number) would have
   silently changed the documented shape — the string + the parser
   keeps the spec shape and gives the wiring a validated conversion.
2. **Zero is refused, not clamped.** A zero window (and a zero
   generation count) is an authoring bug with a silent-no-op failure
   mode; the load refuses with the reason instead of letting the
   machinery expire instantly.

**To unclose.** The F10 CLI adoption (booting the pools from `dist/` +
F11's `releases/` layout, promote on `cmd_dev` reload, the 30 s expiry
task, the F12 brownout trigger calling `evict_oldest()`) and the
`tests/skew.sh` e2e — A01-phase-3 slices on a V8-capable host; the
config key is the face they read.

## V2-F10 — the CLI skew wiring (feat/skew-generation-pool-wire, 2026-11-15)

**What lands.** The id-level half of the "To unclose" list above, in
`crates/cli/src/serve/generations.rs` + the start pipeline:

- `SkewState::from_boot` — the bounded `GenerationPool` the dispatch
  wrapper drives through the same `AcceptedGenerations` seam (the stage
  install and the advertise stamp are the A04 wiring's): the boot's
  build id accepted as CURRENT, the window from the typed
  `deploy.skewWindow` (`parse_duration_secs`; an unparsable literal
  falls back to the spec's 15 m default rather than refusing a serve),
  the depth from `deploy.generations`.
- `promote_from_dist` / `promote_handle_from_dist` — the SIGHUP reload
  step: re-read `dist/build-id`, demote the current build with its own
  demotion instant, accept the new id; an unchanged or unreadable dist
  promotes nothing (an idempotent re-deploy never churns the pool, and a
  serve must not churn on a half-written dist).
- `spawn_expiry_task` — the §V2-F10 step-3 30 s tick: expired demotions
  RECLAIMED (`accepts` already refuses them on read — this is memory
  hygiene, not the verdict).
- `cmd_serve`'s SIGHUP handler (the select over Ctrl-C/SIGHUP, the
  promote logged) and 4 `SkewState` pins (CI-run — the CLI crate links
  V8, so its unit suite rides `cargo nextest run --workspace`).
- `tests/skew.sh` — the CI-executed e2e acceptance (+ its `jshost-e2e`
  CI step): build v1 → start → change the loader → rebuild v2 → SIGHUP →
  the previous id ACCEPTED inside the window, the CURRENT id advertised
  on every data-plane response, an unknown id `409` +
  `x-oxide-build-stale: true`, an HTML navigation never the data plane,
  and past the 2 s window the demoted id refused on read.

**Deltas.**

1. **The e2e pins the ACCEPTANCE contract, not the old generation's
   output bytes.** The spec's step "call the old server-fn id with the
   old header (succeeds, v1 output)" needs the OLD generation's loader
   bytes — which needs the two-pool serving wiring (a second jshost pool
   booted over the old dist, the `GenerationPools<Arc<dyn Pool>>` route
   step handing accepted old-build requests to the old pool). Today ONE
   jshost pool serves every generation, so the old id is accepted but its
   bytes re-execute against the new modules. The two-pool boot is the
   recorded F10 tail; the script documents the delta in its header.
2. **The skew window is config-spelled, not a flag.** The spec's e2e
   names `--skew-window 2s`; the typed `deploy.skewWindow` key (the
   landed §14 duration grammar) is the face the wiring reads — the same
   acceptance, no new CLI surface.
3. **The promote site is `cmd_serve`'s SIGHUP, not `cmd_dev`'s reload.**
   Dev never rebuilds `dist/` (the on-demand ESM server has no dist
   loop), so there is no generation to promote there; production's
   rebuilt-dist promote is the real path. A dev promote joins with the
   two-pool wiring.

**Verification posture.** The dispatch-side verdict pins (set/pool/pools
parity) ran in-session; the CLI crate's own pins (V8-linked) are
compile-verified here and CI-run (`cargo nextest run --workspace`); the
e2e script runs in CI's `jshost-e2e` job. Wired end-to-end — CI
verification pending for the V8-linked halves.

## V2-E12 — the workspace-links substrate (feat/workspace-links-substrate, 2026-11-14)

**What lands.** E12's dev-side substrate — `crates/dev/src/workspace.rs`,
V8-free end to end:

- the injectable `WorkspaceLinks` seam with the production
  `ManifestWorkspaceLinks`: the nearest workspace manifest above the app
  (`pnpm-workspace.yaml`'s `packages:` block or flow list — parsed
  glob-lite, no YAML dependency — or `package.json` `workspaces`, the
  npm/yarn classic shape) owns the candidate set; the link set is the
  candidates the app actually declares as dependencies, resolved through
  `oxc_resolver` with its symlink-resolving defaults (the spec's
  `preserveSymlinks: false`), so `LinkedPackage::real_root` is the REAL
  package tree and never the `node_modules` link;
- the watcher wire-up: `Watcher::with_workspace_links(roots, links,
  app_root)` extends the watch set with each link's `watch_root` (the
  package's `src/` when present, else the package root), deduplicated —
  the spec's "add those real paths to the watcher roots" (§13: an edit to
  `../packages/ui/src` invalidates the app, and unrelated siblings are
  not watched the way the coarse SPEC-16 sibling walk watches them);
- `real_path` — the package-edge bridge the importers-graph harvest runs
  per resolved node: a `node_modules`-arrived path canonicalizes onto the
  real tree so graph nodes and watcher events agree and the boundary
  planner (§V2-E01) walks across the package edge;
- the error contract: a workspace manifest that exists but does not parse
  the two supported shapes refuses loudly (`DEV-WS-001`, registry row +
  reference page row, the fix naming both shapes and the install remedy);
  an uninstalled dependency skips with a warn (the install has not run —
  fatal would be wrong); a broken candidate package.json skips with a
  warn (a broken sibling must not kill the dev boot).

11 TDD pins (`tests/workspace_tdd.rs`: the real-root pin through a
`node_modules` symlink, the watcher-roots pin, the non-workspace and
undeclared-dependency negatives, the candidate-glob isolation, the
uninstalled-dep skip, the loud `DEV-WS-001`, the deterministic order, the
`real_path` bridge and the `package.json`-workspaces shape), the
`workspace_links` criterion group (discovery over an 8/16-package
workspace ≈ 99/153 µs, the `real_path` probe) and the
`workspace_links` walkthrough example ride along; the crate README lists
the seam. `oxc_resolver` rides the pinned workspace version — no new dep.

**Deltas.** None against the spec text; the discovery is glob-lite on
purpose (literal segments and one-segment `*` — the shapes the spec's
fixture and this repo's own `pnpm-workspace.yaml` use), documented in the
module header.

**To unclose.** The CLI adoption — `cmd_dev` replacing the coarse
SPEC-16 sibling walk with `Watcher::with_workspace_links`, the importers
harvest bridging package edges through `real_path`, and the two build-path
halves (`--filter` and the package-name+content-hash CAS key) — all
A01-phase-3 slices on a V8-capable host; the seam is the face they call.

## V2-A02 tail — the odf pedantic pass and the manifest drift (feat/workspace-links-substrate, 2026-11-14)

**What lands.** The recorded "next crate-owned hygiene pass" (the
V2-A02 root-Cargo.toml note: "the remaining V8-free crates adopt pedantic
in a dedicated hygiene pass"):

- `oxide-fw-odf` adopts `[lints] workspace = true` (the `warnings =
  "deny"` + `pedantic` policy) and is clean under
  `cargo clippy -p oxide-fw-odf --all-targets --all-features -- -D
  warnings` — the ~75 pre-existing pedantic findings fixed by
  construction, not suppressed: the wire length prefixes encode through a
  checked `write_len` (`ODF-SIZE` on a `usize` that does not fit the
  wire's `u32` — a truncated length would desynchronize the payload, not
  just shrink it), `i64_le` decodes through `i64::from_le_bytes`, the
  `to_json` integral bridge converts through the guard-bounded decimal
  parse (std has no fallible `f64`→`i64`; the guard bounds |n| ≤ 2^53
  where `f64` Display prints exact integer digits), the missing `# Errors`
  / `# Panics` rustdoc sections exist, the long wire-table functions split
  (`read_record`/`open_container`, `write_typed_array`/`write_error`, the
  fixture tables into section builders — byte-identical output, pinned by
  the golden tests), `bitexact_eq`'s twelve per-type leaf arms collapse to
  one guarded arm (derived `PartialEq` IS bit-exact for every non-Number
  leaf; `Number` keeps its bits arm), and the test/bench casts convert
  through `try_from` with the invariant named;
- the manifest drift the toolchain flagged repo-wide: the unused
  `workspace.package.description` field (every member ships its own
  literal description) and the `homepage.workspace = true` inheritance on
  every member (redundant with `repository` since cargo 1.100's
  `redundant_homepage` lint) — both removed, root and 20 members;
- `oxide-fw-dev`'s unused `oxide-fw-graph` and `rustls` dependencies
  dropped (the toolchain's `unused_dependencies` lint named them);
- the JS minifier's silent `unwrap_or_default()` on `String::from_utf8`
  (`crates/bundler/src/minify.rs`) — a silent empty minified bundle was
  the worst failure mode — now degrades to the unminified source loudly.

**Why no `match_same_arms` exception.** A crate-local `[lints.clippy]`
allow cannot survive the command-line `-W clippy::pedantic` sweep (the
CLI lint level overrides crate tables), so the remaining cross-type
identical-body pairs are restructured instead of allowed: the repo's
zero-`#[allow]` rule holds with no new exceptions anywhere.

**To unclose.** The remaining non-opted V8-free crates (server, bundler,
assets, cache, graph, plugin, core-wasm, dev, cli) — each is its own
recorded pass in the same shape; `server` alone carries ~480 findings.
The `missing [lints]` manifest advisories name them on every build.

## `oxide migrate` codemod planner (V2-E03) — the engine substrate landed, the command is the deferred half

**What the spec asks.** Real `oxide migrate` codemods: a dry-run plan
(`MigrationPlan { renames, rewrites, config, manual }`) per source
framework, `--apply` behind a clean git tree, import rewriting through the
Oxc import-span scanner, per-framework fixture projects with golden plan
JSON, and the SvelteKit/Vite fixtures building after apply.

**What lands (this diff).** The V8-free engine half, in
`crates/compiler/src/migrate.rs` (the crate that owns
`scan_import_spans`; the own-your-files rule): the spec-verbatim
`MigrationPlan` data model, the object-safe `MigrationPlanner` seam with
the four planners (SvelteKit, Next app-router, Nuxt, Vite SPA),
`detect_framework` (package.json dependency names first, the config-file
signatures as the fallback), `apply_plan` (the whole-set collision
pre-check + the rename half + the span-surgery import rewrites), 24 TDD
pins over the four committed fixture projects with reviewed golden
`plan.json` files, the `migrate` criterion bench and the walkthrough
example.

**Deltas.**

- The next.config table values are carried as LOCATED NOTES (file + line
  + the oxide spelling), not extracted values: the planner does not
  evaluate JS, so `images.remotePatterns` plans "map this key, carry the
  value by hand" — guessing a JS value would be exactly the failure mode
  the plan-is-data design exists to avoid. A key the config does not
  carry plans nothing.
- `generateStaticParams` → `generateParams` rides `routes.compat:
  'next'` instead of a per-file identifier rewrite: the router's
  Next-compat scanner (`app/**/page.tsx`) already speaks the
  conventions, and an identifier rewrite is not expressible in the
  four-field plan model — the plan stays pure path/import data.
- The SvelteKit `hooks.server.ts` migration RENAMES to
  `src/middleware.ts` when the file exports a plain `handle` (Oxide's
  one-chain convention) and flags `sequence` compositions manual with
  the merge instruction — the spec's "flag as manual" row.
- `apply_plan` lives in the engine (the CLI wraps it with the
  clean-git-tree gate), so the plan/apply round trip is testable here;
  the config keys and the manual notes are never applied silently —
  the `ApplyReport` carries them pending.
- The spec's acceptance ("the SvelteKit and Vite fixtures build with
  `oxide check`/`oxide build` after apply") is the CLI half and rides
  the V8-capable host, with the F08/F09 acceptance pattern.

**What would change it.** The `oxide migrate` command (A01 phase-3
wiring) — the engine's plan/apply/report surfaces are its contract.

## Offline-first server substrate (V2-F14) — the idempotency stage + the client outbox landed, the PWA worker is the deferred half

**What the spec asks.** The PWA worker with versioned caches, the client
outbox (`packages/oxide/src/client/outbox.ts`), the server
`stage_idempotency` for POST actions and server functions
(`x-oxide-idem` → `idem:<key>` in L1, a hit returns the stored response,
a miss stores 2xx/4xx for 24 h), the typed `offline` config key, and the
matrix row `Offline` Full.

**What lands (this diff).** The server substrate +
the client outbox seam: `crates/dispatch/src/idem.rs` (the
`IdempotencyStore` seam — the L1 tier's shape, object-safe with the
`&T` blanket impl the `AcceptedGenerations` pattern uses; the stage with
the spec's exact verdicts — replay-identical-bytes-handler-runs-once,
different-keys-run-separately, the stored shape `(status, headers minus
set-cookie, body)` with the base64 body the record module's wire
convention, 5xx never pinned, the missing header a pass-through, the 24 h
TTL on the put, a store failure degrading loud-then-run with the
`DSP-116` warn), 12 TDD pins, the `idem` criterion group and the
`idempotency` walkthrough example; the typed `offline:` key (schema'd,
`CFG-OFFLINE` load-validated, the regenerated committed schema); and
`packages/core/src/client/outbox.ts` (the spec-verbatim
`enqueue`/`flush`/`del` over IndexedDB — the id IS the `x-oxide-idem`
key, the delivery rule deleting 2xx + 4xx-minus-408/429 and keeping the
rest, the `online` listener guarded for non-browser imports), 5 node
pins.

**Deltas.**

- The outbox module imports cleanly outside a browser (the spec's
  module-level `window.addEventListener` is guarded), so the node test
  runner loads it directly — the boot wiring rides the CLI adoption.
- The outbox's `flush` processes every item independently (a 5xx item is
  kept and the loop continues — the spec's catch-and-continue shape), so
  one stuck item never blocks the queue behind it.
- The stage returns the first execution's response VERBATIM (Set-Cookie
  included — the browser needs the session); only the STORED copy strips
  `set-cookie`, so the replay never mints a second session.
- The PWA worker half (the precache emitter reading the client
  manifest, the navigation fallback to `/offline`) and the `enhance`
  outbox wire (a failed form submit enqueuing) are the remaining
  halves: the emitter is the V8-linked CLI, the wire is the client
  adoption — the RFN-002 pattern. The matrix row stays Degraded until
  they land.

**What would change it.** The CLI's precache emission + the client boot
wire — the stage's `IdempotencyStore` seam and the outbox's module
surface are their contracts.


## F08 substrate — the native-function host (`feat/rustfn-host-seam`, 2026-11-14)

- `oxide::server_fn` is reachable as `oxide_rustfn::server_fn` (the prelude re-export); the `oxide` facade crate is the CLI/publish half, deferred with the A01 phase-3 wiring.
- `TsType` is a hand-rolled derive in `crates/macros` (named-field structs + fieldless enums), not ts-rs — the substrate needs only the shapes the spec's example carries, and the crate stays dependency-lean.
- The types ride an OPTIONAL `oxide_fn_types_json` side export beside the spec-verbatim `oxide_fn_table` (the FnTable struct itself is unchanged); absent export → no `.d.ts` from that crate.
- `export_table!` is a proc macro (`oxide_macros`), not `macro_rules!`, so the `__oxide_shim_<name>` ident concat needs no paste crate.
- SSR/Edge `.server.rs` stubs emit the call-through shape (isolate-side callers reach the native fn through the §9.4 route); the worker op-table parity (`island:render`-style `A08` pattern) is the remaining half.
- The dev cargo-build/watch loop (`oxide dev` building the user cdylib, hot-swap retire-after-drain) and the production `dist/native/<lib>` emission are the CLI halves — the dispatcher consult, the loader, the registry, the id reconstruction off the types export and the stub generator are landed and pinned.
- The `.server.rs` import resolution at bundle time (the generated stub module reaching rolldown's resolver) is the remaining build half; `rust_fn_stub_source` is the landed generator.

## F01 substrate — the mixed-framework islands planner + client (`feat/mixed-islands-substrate`, 2026-11-14)

- The boot planner is a pure compiler-side module (`oxide_fw_compiler::islands`) over resolved entries; the inference (`island_framework`) lives in the config crate per the spec; the CLI does the file reads and delegates. Single-framework output is byte-preserving (the legacy shape), mixed takes the registry shape.
- The adapter import is the package root (`@oxide-fw/<framework>`); the rolldown `advancedChunks` per-framework vendor split is the remaining build half.
- The foreign-island SSR placeholder is the React adapter's `<Island>` (`data-ox-fw`/`data-ox-pending` attributes land with the adapter work); the core `resolveForeign` transform and the mixed client registry are landed. The Vue/Svelte/Solid `island.ts` twins and the worker `island:render` op (A08 parity) are the remaining halves.
- The conformance assertion `mixed islands` and the Playwright four-island click-through ride E10/E10-adjacent work (documented, not silently stubbed).

## Offline-first tail — the PWA worker emitter + the enhance outbox wire (`feat/pwa-precache-emitter` + `feat/offline-enhance-outbox`, 2026-11-14)

**What lands.** The two V8-free halves the F14 substrate record deferred.
`crates/bundler/src/pwa.rs`: the object-safe `ShellSource` seam (the
prerendered route shells, as the URL paths a navigation requests), the pure
`plan_precache` over the typed `sw:`/`offline:` keys — the build-id-versioned
cache name `ox-<build-id>`, the `offline.precache` breadth's own docs
implemented verbatim (`routes` = exactly the shells, `all` = every hashed
manifest asset, the shell rides both because the navigation fallback needs
it), the `SwConfig` comment's fallback-resolution contract enforced
(`sw.offlineFallback` override → `/offline` when the build produced it →
`/`), the deterministic `sw_source` render (install `addAll(PRECACHE)`;
activate deletes the stale `ox-` caches and, when `offline.outbox` holds,
fans `OXIDE_OUTBOX_FLUSH` to every client — a build swap is a flush point;
`/__data` serves stale-while-revalidate; navigations fall back to the
resolved route shell or the network; hashed `/client/` assets are
cache-first) and `emit_pwa`'s `dist/sw.js` write (the manifest-write
convention). And the client wire: `enhance` gains `outbox`/`isOnline` — a
submit the browser knows cannot reach the network (the probe, or the fetch
`TypeError`) enqueues into the outbox with the faithful request headers
(`accept`, the urlencoded `content-type`, `x-oxide-build`), dispatches
`ox:form-queued` with the id, and `outbox.onSwFlushMessage` answers the
worker's fan-out with a queue flush. 14 Rust + 10 TS pins, the `pwa`
criterion group (plan ≈ 360 ns at 8 shells, ≈ 1.1 µs at 32; the source
render noise-scale), the `pwa_worker` walkthrough example and the
`enhance-offline` TS micro-bench ride along.

**Deltas.**

- The emitter lives in the bundler crate beside the `ClientManifest` it
  reads — the spec's `emit/pwa.rs` spelling was the old CLI layout (the
  own-your-files rule; the precompile-engine precedent). The CLI call-site
  is one line over the produced manifest (`offline.enabled && sw.enabled`
  is the gate the config docs state; the module itself is total).
- The worker's outbox arm is an ACTIVATE fan-out (`OXIDE_OUTBOX_FLUSH`
  postMessage to all clients), not an in-worker flush: the outbox module is
  page-context code, and the SW-side replay would need the outbox chunk's
  hashed URL. The client listener (`onSwFlushMessage`) is the consumer; the
  boot's adoption of it rides the CLI wiring.
- Multipart form bodies cannot ride the string-item outbox — they keep
  today's failure path (the parts have no string shape to store); recorded
  in the `enhance` docs and pinned.
- The offline probe counts as offline only when the platform SAYS offline
  (`navigator.onLine === false`); Node's missing `onLine` counts as online,
  so the existing enhance pins stayed byte-identical.
- The seam fix riding this diff: `resolveForeign`'s splice produced a
  double space where `data-ox-pending='1'` sat between the two captured
  attribute sides (the strip regex targeted a flag the greedy match had
  already consumed) — the pre-existing red pin
  (`foreign-islands.test.ts`) is green again with the seam collapsed to
  one separator.

**What remains.** The CLI build call-site (passing the loaded `sw`/`offline`
config + the prerender walk into `emit_pwa`) and the boot's
`onSwFlushMessage` adoption — both thin wrappers over the landed seams (the
A01 phase-3 wiring on a V8-capable host). The `Offline` matrix row stays
Degraded until they land.

## Offline-first wire-through — the CLI emission, the boot registration + the flush listener (`feat/pwa-boot-wire`, 2026-11-15)

**What lands.** The F14 tail the two records above deferred — every
remaining call-site, wired; nothing new sits behind a seam. The CLI
build's `emit_pwa_worker` (`crates/cli/src/lib.rs`) drives the bundler
seam at the gate the config docs state (`offline.enabled && sw.enabled`
over the produced client manifest — `offline.enabled = false` installs no
worker, silently, because that is the default posture; `sw.enabled =
false` refuses loudly, a configured-but-refused feature is worth a line;
a manifest-shape mismatch is loud, a bundler/CLI contract bug), with the
call-site `PrerenderedShells` walk implementing the `ShellSource` seam
over `dist/prerendered/**.html` in the navigation URL spelling
(`url_path_from_rel` — the sitemap walk's ground-truth rule, shared
verbatim). `emit_pwa` writes BOTH dist files now: `sw.js` and the
registration snippet `pwa-register.js` (`pwa_register_source`,
deterministic — a page cannot register the worker without it, so the two
files exist together). The boot-entry generator's registration injection
rides the SAME gate at both callers (it was `cfg.manifest.is_some()`:
registering a worker the build did not emit 404s, emitting one no page
registers is dead). The boot module (`packages/core/src/client/boot.ts`)
answers the worker's activate fan-out: a module-scope, page-lifetime
`onSwFlushMessage` listener (guarded for the SSR/config-loader import; a
no-op without a service-worker registration) replays the queue when
`OXIDE_OUTBOX_FLUSH` arrives, and `flush` resolves empty instead of
rejecting when the store is unavailable — the fire-and-forget trigger
sites (the `online` listener, the fan-out) never surface an unhandled
rejection. 6 CLI pins (the gate ×2, the seam happy path, the shell-URL
spelling, the `all` breadth through the CLI, the offline fallback), 2
bundler register pins, 4 TS pins (the boot listener ×3 + the flush
no-op); the `pwa_worker` example walks the register emission too.

**Deltas.**

- The old CLI worker body (the hand-rolled SWR source over the JSON
  snapshot, cache `oxide-<build>`, network-first navigations) is DELETED —
  the superseded path; the bundler seam's worker (`ox-<build-id>`,
  stale-while-revalidate `/__data`, the navigation fallback chain) is the
  one implementation. The webmanifest emission (`emit_web_manifest`) is
  the §16.3 piece and keeps its own `manifest:` gate — unrelated.
- The boot listener is module-scope (evaluated once per document), not
  wired into the `boot()`/`bootWithRouter` bodies: it is page-lifetime glue
  exactly like the outbox module's own `online` listener, and the router
  disposer does not own it — a torn-down router still receives the
  worker's message. Module evaluation happens at most once per document,
  so the listener cannot double-register.
- The registration snippet's source is byte-identical to the old CLI's
  (shipped pages keep their cache semantics); the gate that decides
  whether it EXISTS is what changed.
- The `SwConfig.enabled` doc comment claimed "default true when
  `manifest` is present" — that gate no longer exists; the comment now
  states the §V2-F14 gate (`offline.enabled` + `sw.enabled`).
- The `Offline` capability-matrix cell was already `F` (the R02 rows
  carried their target from day one); with the wire-through the declared
  cell is BACKED by code end-to-end. The e2e execution (a real PWA build
  through `oxide build` with a served page) rides CI — every CLI pin in
  this milestone is compile-verified (the sandbox `cargo check -p
  oxide-fw-cli --all-targets` is green) but runs on CI only (the
  V8-linked link step).

**What remains.** Nothing for F14 at the code level: the module, the
config, the server stage, the client wire, the boot listener and both
CLI call-sites are landed. CI executes the CLI pins + the conformance
sweep.

## Auto-islands inference (V2-F02) — the scanner + manifest + gate landed, the CLI call-site is the deferred half (`feat/auto-islands-inference`, 2026-11-14)

**What landed.** The §V2-F02 one-week-spike's V8-free substrate:
`crates/compiler/src/interactivity.rs` — the spec-verbatim
`scan_interactivity(source, framework) -> Interactivity` (direct detection
only): React (`useState`/`useReducer`/`useEffect`/`useLayoutEffect`,
`useRef` **with** a `.current` write, any `on*` JSX attribute), Solid
(`createSignal`/`createEffect`/`createStore`, `on*`), Vue (`ref`/`reactive`/
`computed`/`watch`/`onMounted`/`emit`, `v-on:`/`@event` in the template),
Svelte (`$state`, `$:` labeled statements, `on:`/`onclick`/`bind:`); the
`{ file: { interactive, reasons } }` manifest (`IslandManifest`, the
`.oxide/island-manifest.json` side-car contract, compact + sorted + a
typed `AST-ISL-001` refusal on a corrupt read); the project fixpoint
(a component is pure when it has no hits AND imports no interactive
component from the same project — relative specifiers only, transitive);
and the hydration decision (`hydrates`: an explicit registry entry always
wins, a pure classification is the only skip, missing data hydrates). 27
TDD pins, the `interactivity` criterion group, the `auto_islands`
walkthrough example, and the typed `framework.autoIslands` gate (schema'd,
`KNOWN_KEYS` face unchanged — the key nests under `framework` — and the
committed schema regenerated). Every conservative arm points the same
direction the kill criterion demands: an unparseable module classifies
interactive, a missing classification hydrates, an over-flagged island
only costs bytes — a pure classification is the ONLY reason hydration is
skipped, so the worst case of a wrong inference is today's behavior.

**Deltas.**

- The gate is `framework.autoIslands`, not the spec's
  `experiments.autoIslands` spelling: F04 already typed `experiments` as
  the A/B array (a different shape), so the flag lands beside the islands
  registry it governs. The default is `false` — the spec's own default
  ("a wrong inference silently breaks interactivity, so the default is to
  defer").
- The Vue/Svelte template arms are a conservative token scan over the
  `<template>` region (the script arms are real Oxc parses), not a full
  template parse — no Vue/Svelte template parser exists in the Rust tree.
  The scan can only over-flag (`@media` in a stray inline style, an email
  in text); it cannot under-flag, so the kill criterion stays safe.
- `defineEmits`/`emit` (Vue) and `$effect` (Svelte) ride the detection
  sets beside the spec's named tokens — an emitting or effecting
  component is interactive by construction; the superset is the safe
  direction. `$state.raw(...)`/`$effect.pre(...)` count under the rune.
- A module that does not parse classifies `interactive` (`unparsed`) —
  content the parser cannot read is content the scanner cannot vouch for,
  and a false negative silently breaks interactivity.
- The manifest write and the registry merge stay at the CLI call site
  (the F04 ruling: the module never touches the filesystem; the
  `routes.reasons.json` advisory precedent). The dev warn for a
  pure-classified component receiving `on*` props needs the island props
  at the boot/render surface and rides the same wiring.

**What remains.** The CLI build call-site: when `framework.autoIslands`
holds, scan the client tree into the manifest side-car and merge the
interactive files into the boot registry before `plan_island_boot` (the
A01 phase-3 wiring on a V8-capable host). The `MixedIslands` capability
row is unaffected; the spike's 15%-JS-saving measurement runs after the
call-site lands (the scanner is deterministic and V8-free — the build-time
side of the spike is done).

## V2-C01 tail + C03/C04/C05/C10 + C11 — the Wave C batch (2026-11-14)

**What landed.** The C01 tail (the CLI adoption): `compile_one` consults
`NativeRegistry::pick` ahead of the pool branch on `spawn_blocking` (the
registry rides an `Arc`, cloned into the blocking closure — no
`unsafe`), `PrecompileKey::tool_version` folds `cas_tool_version()`
(`native:{id}@{version}` — D40) for the native arm, the build
fingerprint folds the `framework.compilerBackend` selection
(`build_cache::compiler_backend_hint` — the `OXIDE_COMPILER_BACKEND`
env layer the file walk cannot see), `oxide doctor` prints one
`native compiler <id> <version> ok|unhealthy` line per registry entry
and fails when `compilerBackend: 'native'` has nothing compiled in, and
`CMP-NAT-001` refuses end-to-end (`precompile_entries` fails the build
under the `native` hard request instead of degrading to the pool). C03
(`verter_core` 0.0.1-beta.1 behind `native-vue`), C04 (`svelte-compiler`
0.1.4 behind `native-svelte`) and C05 (`oxc_react_compiler` 0.143.0
behind `native-react-compiler`) landed as native backends with spike
tests, criterion groups and the `scripts/compiler-equiv.sh` gate. C11
landed `check.typecheck` (typed, schema'd, `CFG-CHECK`, KNOWN_KEYS,
regenerated schema), `oxide fmt [--check]` (oxfmt → biome format → loud
SKIPPED, `OXIDE_CHECK_STRICT=1` fails), `resolve_tool` (node_modules/.bin
→ PATH), the doctor tool inventory and the fake-executable test suite.

**Spec deltas (recorded, not silent).**

1. **The brief's §0a voids D27, so C02 is killed where the spec sketches
   vendoring.** `docs/decisions/C02-solid-kill.md`: the crate does not
   exist on crates.io and the solidjs/solid monorepo carries zero
   `Cargo.toml` — unpublished, unmirrored, nothing a `rev` can pin.
2. **No feature flag without a backend behind it** (§0d): the brief's
   C01 bullet asked for all four forwarded features; only the three
   landed backends' features exist (`native-vue`, `native-svelte`,
   `native-react-compiler`) — a `native-solid` flag with no code
   reading it is a gate failure, not scaffolding.
3. **`precompile_entries` returns `Result` now** (additive): the
   `native` hard request must fail the build (`CMP-NAT-001`), while
   `auto`/`node` keep the §6.3 best-effort warn-and-skip per file.
4. **`verter_span` is lockfile-pinned to 0.0.1-beta.1**: the published
   beta line is internally skewed (beta.6 builds against oxc_span 0.151,
   verter_core beta.1 against 0.117 — 101 errors in the published
   pair); the `--precise` pin unifies 0.117. A workspace
   `[dependencies]` entry cannot override a transitive requirement.
5. **C05's oxc skew rides package-renamed aliases** (`oxc_parser_0_143`
   et al.): one `[dependencies]` table cannot carry the same crate
   twice, and the workspace's 0.155 stays the transform's stack. The
   string-in/string-out seam is what makes the two majors coexist.
6. **C10 is spiked-killed** (`docs/decisions/C10-mdx-kill.md`): the
   candidate emits a JSX module — adapter neutrality (the spec's own
   condition (a)) fails for the Vue/Svelte adapters. No code change.
7. **The dev server's `dev_component_sources` stays pool-only** for
   now (the C01 seam's build-path adoption; the file list the WP owns
   does not include the dev-shim wiring) — recorded so the gap is
   explicit.
8. **Baseline repair (pre-existing breakage at the starting commit,
   not WP work, all recorded):** nine stale `oxide_fw_rustfn` imports
   in `crates/cli/src/lib.rs` (the crate is `oxide-rustfn`); the
   islands migration left `generate_island_boot_entry` on
   `BTreeMap<String, String>` while callers pass the typed
   `IslandSpec` registry, and the `islands_json_source` helper the
   call site references was never added (added: it resolves each
   entry through `island_framework` and delegates to
   `plan_island_boot(..).islands_json()`); two discovery-test
   fixtures wrote files into directories that did not exist and
   imported components with wrong relative paths (the walk is
   import-driven — the fixtures now import what they assert on).
9. **The typecheck-tool runner is a helper** (`run_tool`) so the fake
   executables can drive the OK/FAILED outcomes without a real
   toolchain on PATH; the `OXIDE_COMPILER_BACKEND` arm of
   `compiler_backend_hint` is pinned by the config suite instead of a
   mutating test (edition-2024 `set_var` is unsafe and parallel tests
   race env reads).

**To unclose.** The equivalence harness's Playwright leg (the island
click spec) runs in CI's jshost-e2e (browsers installed); the cold-build
≥3× acceptance number needs the release build with the features on a
dedicated machine — the criterion groups carry the per-backend
compile-time shape meanwhile.

## V2-C02 — the Solid native compiler re-open (2026-11-14, `feat/native-solid-backend`)

**What landed.** The kill record (`docs/decisions/C02-solid-kill.md`) set
re-run conditions; the re-measure found the condition met in spirit —
`@solidjs/compiler` 2.0.0-rc.21 on npm is real and official (the Solid
2.0 RC's default Oxc JSX compiler, the Node `transform()` contract the
supported surface) — while the dependency arm stayed unmet (crates.io
404s `solidjs-compiler`; the `solidjs/solid` `main` tree carries no
`packages/compiler`/`Cargo.toml`, so nothing a `rev` can pin). The WP
re-opened with the **in-repo transform arm**: `crates/compiler/src/native/solid.rs`
(`SolidNative`, behind the new `native-solid` feature — no external
dependency rides it), implementing the worker oracle's contract over the
workspace oxc 0.155 stack: parse (tsx/jsx) → the TypeScript pass (JSX
preserved; `only_remove_type_imports` keeps unused value imports — the
sucrase parity) → the JSX walk → the `@solidjs/web` emission (DOM:
hoisted `template` constants with `<!>` markers, `firstChild`/
`nextSibling` walk binds, `insert` before markers, `setAttribute` +
the small known-property set, `$$evt` delegated handlers + the module-tail
`delegateEvents`, `createComponent` getter props, DOM fragments as
arrays; SSR: `ssr` template literals, `escape` text holes,
`ssrAttribute` with the known-boolean set, `ssrHydrationKey` after every
tag name, components interpolated, client-only constructs dropped with
warnings) → one codegen print → the worker's `solid-js/web` →
`@solidjs/web` import rewrite. The registry pushes the backend
(`register_compiled_in` — the push list moved into a helper fn, which
also removed the file's `#[allow(unused_mut)]`), the CLI forwards
`native-solid`, the release recipe compiles it in.

**Spec deltas (recorded, not silent).**

1. **The vendoring arm is void; the transform is in-repo.** Nothing is
   sourceable at upstream `main`, so `vendor/`/`UPSTREAM_REV` do not
   exist and `version()` is this transform's own revision (`1.0.0` — the
   D40 CAS dimension to bump on any output-shape change), not an
   upstream rev.
2. **The spike file is `solid_native_spike.rs` and NOT `#[ignore]`d**
   (the spec's `solid_native.rs` + `#[ignore]` sketch assumed a Node
   oracle comparison inside the test): the transform is in-repo, so the
   25 shape pins run under the feature with no Node host; the
   Node-oracle equivalence stays with `scripts/compiler-equiv.sh solid`
   (the capable-host acceptance, the C03/C04 shape).
3. **The pre-C02 registry pins moved to an `empty()` base** (`tests/native_registry_tdd.rs`):
   they hold the SELECTION ENGINE over injected fakes, and
   `from_enabled_features()` stopped being empty the moment real
   backends register — a real `pick("solid", "tsx")` would shadow the
   fake a pin drives (this also repaired the two pins that were red at
   the starting commit under `--all-features` — the empty-registry
   premise pre-dated the C03/C04/C05 landings). The feature-to-registry
   mapping is pinned feature-aware in the lib tests
   (`from_enabled_features_matches_the_compiled_in_set`).
4. **The compiler-equiv solid arm rides `scripts/compiler-equiv.sh`**
   (the smoke app renders through the pool and the native backend; the
   Playwright island leg stays the CI jshost-e2e surface) and the
   ignored `compiler_equiv_solid` integration pin joins its siblings —
   both need the V8-linked release CLI this sandbox cannot build, so
   they ride the capable-host gates like every prior backend's arm.
5. **Byte fidelity vs babel-preset-solid is not claimed** — the emitted
   shape is runtime-correct for the pinned subset (the tests pin THIS
   backend's deterministic output), and the rendered-HTML equivalence is
   the harness gate. The hydration contract mirrors the worker's
   `hydratable: true` in the SSR shape; the DOM shape is
   hydration-agnostic.
6. **The v1 refusal scope** (`CMP-SOL-001`): spreads, `ref`, `use:`,
   `oncapture:`, object-literal `style`/`classList` (string forms are
   supported), `innerHTML`, SSR fragments, unparseable sources — each
   refusal names the construct and the Node-pool fix; `auto` falls back
   silently, `native` fails the build, matching the C01 contract.

**Micro-debt and drift in the same diff.** The compiler crate's only
`#[allow]` (`unused_mut` on `from_enabled_features`) is gone (the
push-list helper shape); the stale module docs ("What is compiled in
today — nothing yet", the C02 kill references) updated to the four
landed backends; the missing `CMP-VUE-001`/`CMP-SVT-001`/`CMP-RCC-001`
registry rows added (the codes existed in code with no registry entry —
drift) plus `CMP-SOL-001`; the `native_registry` example's stand-in
comment ("a stand-in for a real C02 backend") updated to name the landed
backend; the spec status table's C01 row (partial → merged, the tail
recorded) and the C02–C11 group row (partial → merged, per-WP records).

## V2-C02 — the dependency arm lands: the in-repo transform swaps for Solid 2.0's official Rust compiler core (`refactor/solid-upstream-compiler`, 2026-11-15)

**What changed.** The re-open record (`docs/decisions/C02-solid-native.md`,
2026-11-14) set the re-run condition for the day upstream published a
sourceable Cargo tree. The `solidjs/solid` `next` branch now carries
`packages/compiler` — the `solidjs-compiler` crate behind npm
`@solidjs/compiler` 2.0.0-rc.21 — so the condition is met and the
dependency arm landed: the 1.7k-line in-repo JSX/reactivity transform is
deleted, `crates/compiler/src/native/solid.rs` is a thin adapter around
`solidjs-compiler` (git-pinned at full SHA
`7ea2a02e9edc976e4be4d060c7e65ba85d10b42c`, `default-features = false`
— no NAPI, no tsrx fork), and
the workspace oxc 0.155 stack stays the only oxc the seam's types see
(the upstream crate pins its own oxc 0.144 internally; string in /
string out, no type crosses). The D40 CAS version moved 1.0.0 →
`2.0.0-rc.21+7ea2a02`, so every artifact the in-repo transform emitted
invalidates.

**Deltas vs the re-open's v1 (recorded, not silent).** The TS strip runs
BEFORE the upstream compile (the core parses `.tsx` but does not strip
annotations — the worker strips around its transform too; stripping
first keeps the upstream source map valid). Dev builds now carry the
upstream `sourceNames` markers, keep the `<Show>` component unlowered
and emit the upstream V3 source map through a new `CompileOutput.source_
map` seam field (the first native backend to emit one; the production
pipeline does not consume per-file native maps yet — the pool emits
none). The v1 refusal list shrank to what the 2.0 core itself refuses:
spreads, SSR fragments, object `style`/`classList` and `innerHTML`
compile natively now; `use:` directives refuse (removed in Solid 2.0)
with the upstream migration note; every upstream failure still maps onto
`CMP-SOL-001` with the Node-pool fix. The spike pins
(`solid_native_spike.rs`) hold the upstream emission shapes and byte
determinism; `scripts/compiler-equiv.sh solid` stays the capable-host
parity gate.

## V2-F08 — the dev cargo-build/watch half lands (`feat/server-lib-dev-loop`, 2026-11-15)

**What landed.** `crates/rustfn/src/swap.rs` — the swappable host state the
dev loop drives: `SwappableHost` (a `RustFnHost` behind a short mutex) with
the swap verb (mount the rebuilt library, re-point every §9.4 binding under
it, UNREGISTER the ids the new bindings no longer name — the consult's
JS-pool fallback takes a removed export back), the two-phase `call_now`
(resolve the library `Arc` + export name under the lock, run the call
OUTSIDE it — a swap never waits on a call), and `retire_idle` — the spec's
retire rule enforced: a mount an id still references is never retired, a
mount an in-flight call still holds (`Arc::strong_count > 1`) survives the
pass and is reclaimed by the next one. The host primitives it drives landed
on `RustFnHost` (`unregister`, `resolve_call`, `retire_idle_mounts` with the
index re-compaction). `crates/dev/src/server_lib.rs` — the loop: the watch
set is the spec's exactly (`**/*.server.rs` + `Cargo.toml`, never `target/`),
one poll-shaped step per tick (no internal threads — the dev task's interval
drives it), build → load → bindings → swap, every seam injectable
(`LibBuild` / `LibLoad` / `ServerLibSwap`; production `CargoLibBuild` runs
the spec's `cargo build --lib` and refuses with `DEV-RFN-001` carrying
cargo's stderr tail; `CopyLoader` is the host's copy-then-load; `HostSwapper`
drives the swappable host). The §9.4 id bindings re-derive through the ONE
`server_fn_id` definition the compiler crate owns (§9.4, D6) — the loop
never re-implements the digest; a missing or corrupt types payload refuses
`DEV-RFN-002` LOUD (a zero-binding swap would silently drop every id). A
failed step refuses and the previous mount keeps serving — a broken rebuild
never takes the running dev server's native fns down.

**Deltas.** (1) One `SwappableHost` per native crate (the swap's
drop-by-bindings semantics are per-crate — a shared host would drop another
crate's ids; the consult adapter consults each host in turn). (2) The loop's
mount dir defaults to `<crate_dir>/.oxide/rust`; the CLI adoption passes the
app root's (the consult's mount walk) and the build secret. (3) The retire
pass also keeps the boot mounts referenced — the reference check is
defensive over the swap-only shape (through the swap API the newest mount is
always the referenced one). (4) The `reg` all-off arm of
`NativeRegistry::from_enabled_features` gained a compiled-away use: this
rustc (1.100) lints an unused `&mut` parameter under `unused_variables`, and
the previous session's "never dereferenced mutably is not a lint finding"
carve-out no longer holds — the default-feature `cargo check -p
oxide-fw-compiler` was RED at HEAD in this environment (baseline repair, no
`#[allow]` escape).

**Tests.** 9 swap pins (`crates/rustfn/tests/swap_tdd.rs`: re-point + new
behavior, drop, add, the sorted report, the two-phase call, the in-flight
retire pin with a REAL blocked call, the orphan-only reclaim, the Send+Sync
handle, the generation counter) + 11 loop pins + 2 `#[ignore]` real-cargo
pins (`crates/dev/tests/server_lib_tdd.rs`: the broken-crate `DEV-RFN-001`
refusal and the full fixture loop — real `cargo build`, real copy, real
swap, the rebuilt `thumbnail` answers through the re-derived §9.4 id) + the
`server_lib` criterion group (the quiet-tick poll cost over 8/32 files, one
full fake step, the swap at 4/16 bindings, the bindings derivation, the
event filter) + the `server_lib_loop` walkthrough example.

## V2-F09 — the client matcher lands (`feat/search-client-matcher`, 2026-11-15)

**What landed.** `packages/core/src/client/searchEngine.ts` — the spec's
`import { search } from 'oxide/search'`: the server mode rides
`window.__oxide.search === 'server'` (the boot flag's type extended) and
calls `GET /_oxide/search` with the endpoint's wire rules (limit default 10
/ cap 100, empty-string filters = absent, a non-200 throws with the status);
the static mode lazily loads `dist/search/static.json` (default
`/search/static.json`, exactly once — the fetch and injected loaders both
memoized) and runs the server's `StaticMatcher` arithmetic VERBATIM:
postings hits, the title/excerpt prefix arms with the max-3 title boost,
the score sort with the doc-order tiebreak, the limit take, the post-take
tag filter. An empty/whitespace query refuses locally with an empty result —
the server would 400 `SRC-104`, but a UI's empty input box is a normal
state, so the client never fetches for it.

**Deltas.** (1) The tag filter's landed post-take position is PINNED, not
"fixed": the parity contract (the server's own StaticMatcher parity test)
only holds if both implementations run the same arithmetic, quirk included;
a narrower tag can return fewer hits than the limit allowed. (2) The
injected loader seam memoizes per function identity (a WeakMap) so once-
semantics is uniform across the ready-index, loader and fetch paths. (3)
The client does not ship a UI — the spec defers the island's markup to the
user; this module is what every such island calls.

**Tests.** 13 node pins (`test/search-engine.test.ts`: the tokenize rules,
the title/body/excerpt scoring arms, the prefix postings, the sort + doc
tiebreak, the limit/total split, the post-take tag quirk both ways, the
local empty-query refusal with a fetch spy, the wire params, the boot flag,
the once-lazy load, the 429 throw, the malformed-export throw) + the
`search-engine.bench.ts` micro-bench (match/100-docs ≈ 458 ns/op, tokenize
≈ 679 ns/op).

**Micro-debt in the same diff.** `crates/rustfn/src/types.rs`'s
`types_json` serialize failure wrote an EMPTY types payload silently — the
boot id reconstruction would register ZERO native fns with no diagnostic
(the silently-empty-write shape). The `String` return is the export
contract (`export_table!` emits the call), so the failure is now a loud
`RFN-004` error naming the serde cause, the fallback unchanged.
`crates/dispatch/src/replay.rs`'s two base64 `unwrap_or_default()`s
misdiagnosed a corrupt record: a truncated request body compared as "not
the recorded call" (`RPL-002`) and a truncated response body reported a
spurious body divergence — both now surface the corruption (`RPL-006`, new
registry + reference rows) instead of blaming the code path.

## The F07 + F12 wiring lands — the A01-phase-3 deferrals close (`feat/f07-resume-wire` + `feat/f12-brownout-wire`, 2026-11-15)

**F07.** The four deferrals listed above are wired: the tee in the dispatch closure's
endpoint arm (a streamed response marked through the new `oxide/server` `resumable`
helper is teed under `stream_id(request_id, counter)`; the recording tee composes
OUTSIDE the stream tee, so the store and the recorder observe the same bytes in the
same order), the `GET /__oxide/stream/<id>` reattach arm (GET-only, `parse_range_offset`
with the absent-header default, `stream_resume_response`/`stream_gone_response`; the
per-IP reattach limit rides a dedicated sharded `RateLimiter` bucket — 60/min, `429` +
`retry-after` — so reattach pressure never shares a bucket with the search/OG
endpoints), the store sweep task (`stream_store::spawn_sweep`, the spec's 30 s cadence,
spawned at every serve boot over the typed `stream.resume` bounds via
`serve::streams::StreamWiring::from_config`) and the client lifecycle
(`fetchRouteData` rides `resumableFetch`; a stream-marked `/__data` response is pumped
through the reattach protocol and reassembled for the ODF decode, unmarked responses
keep the buffered read byte-identically; `resumableFetch` is exported from
`@oxide-fw/core`, `resumable()` from `@oxide-fw/core/server`).

**F07 delta (the e2e's producer shape).** The tee is pull-driven: a streamed producer
only reaches the store while SOME consumer keeps pulling, so an e2e whose first client
dies mid-stream needs the whole body pulled before the drop. The fixture emits the
twenty lines in two eager batches (no inter-batch timer); the spec's timed-producer
shape (20 lines at 50 ms) is pinned V8-free at the substrate level
(`reattach_stream_emits_backlog_then_live_tail_until_done`), where a live reader drives
the tail. The drop+reattach acceptance itself (kill after 8 lines → `Range: bytes=<n>-`
→ the concatenation byte-for-byte) is the `#[ignore]` pin
`stream_resume_reattach_delivers_the_missed_bytes` + its CI step.

**F12.** The sampler (a 500 ms tick over `pool.stats()`'s queue-wait p99 ms→µs and
in-flight, the 100 ms loop-lag probe's overshoot atomic, RSS from
`/proc/self/statm` via `rss_mb()`), the enforcement stage in the dispatch closure after
the route match (the route's `config.priority` × `serve_decision`; `Shed` answers
`shed_response` BEFORE any loader/cache/render work; `ServeStale` arms the L0 lookup
with the `staleCeiling` bound — `L0Cache::get_brownout`, the bound math in exact u128 —
and stamps `warning: 110 - "Response is Stale"` on the served hit), the readiness flip
(`/_oxide/ready` answers 503 only at `Emergency` through `ready_status`, both ops
endpoints report the level as `x-oxide-brownout`), the scanner reading
`config.priority` into the route tree (the same best-effort literal contract as
`live`), and the e2e (`brownout_sheds_low_and_reports_the_level`, the governor's level
driven through the SAME seam the sampler writes) + its CI step.

**F12 deltas (2).** (1) The `Emergency` L0-hit/static-asset exemption ("only L0 hits,
static assets and critical routes") is NOT implemented: the enforcement stage runs
after the route match and BEFORE the cache lookup, so a route's cacheability is not
yet known at the shed point — `Emergency` sheds `low` and `normal` routes entirely
(the safe-side shape). The `governor::serve_decision` doc already reserves the
exemption as the caller's; wiring it means the shed decision moves behind the L0
lookup, which changes the hit path's hot shape. (2) The `rules` `NativeRule.priority`
config surface is plumbed (the CLI already transfers it) but the per-URL rule priority
is not consulted at enforcement time — the route tree's scanned `config.priority` is
the declaration the stage reads; a rule-declared priority for a URL without a
page-level declaration falls back to `normal`.

**Deferred (unchanged):** the hedger (opt-in, off by default), the `overload.sh`
`oha -c 500` load test (an ops script — its number is CI-measured), and the F10
previous-generation early shrink (the brownout consults
`oxide_pool_queue_wait_p99_us` against the F10 expiry task).

**A01 phase 1 (the MAP re-derivation and the mover's mechanical extensions).** The
spec-embedded MAP targeted the 7,696-line tree's 85 named items; today's
`crates/cli/src/lib.rs` is 19,308 lines with 285 top-level items, so
`scripts/split_cli_MAP.py` re-derives the MAP: the spec MAP verbatim (two names the
F-passes retired from the tree — `is_secret_key`, `log_request` — dropped, recorded here
as the stale-MAP delta), extended with rule buckets for every new family (the same
`<family>/<concern>` naming the embedded MAP uses; 37 modules total) plus
anchor-nearest placement for helpers, and the 17 `impl` blocks moved span-keyed next to
their structs (the spec's note (c) hand-move, done mechanically). The MAP covers every
named movable item (223) and every impl — `missing: [] duplicates: [] unmapped: []` in
plan mode; the 45 items that stay in lib.rs are the crate-root surface itself (the 22
`mod` declarations, 13 `use` items, 5 type aliases, 2 statics, 2 consts, 1
`macro_rules!`). Mover extensions the current tree forced (all mechanical,
all recorded): (1) `pub use X::*;` instead of the spec's `pub(crate) use X::*;` — a glob
re-export caps at each item's own visibility, so per-item visibility is preserved
exactly (`main.rs`'s `oxide_fw_cli::Cli`/`run`/`apply_startup_env` stay pub; the
`pub(crate)` spelling would have broken the bin crate), with the `pub(crate) use`
spelling for modules holding nothing pub (a `pub use` glob over only-private items warns
`unused_imports` under `-D warnings`); (2) attribute-block continuation tracking in
`split_items.py` so a wrapped `#[command(...)]` attr block moves WITH its struct (the
spec indexer dropped indented attr continuation lines); (3) field-level `pub(crate)`
bumps where a moved struct's private field is constructed/read cross-module
(`HmrModuleGraph.importers/entries/accepting`, `PrerenderedShells.urls`,
`PrerenderTarget.rel/url_path/params/locale`, `DispatchOpts`'s 19 fields,
`PrerenderedShells::collect`) — E0616/E0451/E0624 are the spec's own "expected compile
fixes (all mechanical)" bucket, and each bump is a pure visibility change the selfcheck
normalizes; (4) `pub(crate) use outln;` after the macro definition — `serve/mod.rs` is
textually before the macro, so `serve/start.rs` could not see it under the macro's
textual scope; the re-export makes it path-importable so the `use crate::*;` globs reach
every module; (5) three modules whose items reference nothing from the crate root
(`build/secret.rs`, `observability.rs`, `emit/assets.rs`) dropped the unused
`use crate::*;` preamble (the only alternative is a new `#[allow]`, which the zero-
suppression rule forbids). `split_selfcheck.py` normalizes the visibility bumps and
rustfmt's re-layout (whitespace removal + trailing-comma-before-closer; the `pub(crate)`
prefix re-wraps signatures past the line width and re-adds trailing commas) and proves
all 240 moved items token-identical to `git show HEAD:crates/cli/src/lib.rs` — zero
diffs. The digest proof rides the new `cli-split-digest` CI job: it builds each of the
four smoke apps at the pre-split base commit AND at HEAD and diffs the four
`scripts/dist-digest.sh` values — the sandbox cannot build the V8-linked CLI, so the
literal four-digest baseline is measured by CI's base step (the spec's Step 0 numbers)
rather than pinned by hand here.

**M2 drift cleanup (the post-phase-1 hygiene pass).** The seven clippy findings the
phase-1 delivery inherited (the `-p oxide-fw-cli` gate was red at pristine HEAD) are
fixed, and the gate is now GREEN (`cargo clippy -p oxide-fw-cli --all-targets --no-deps
-- -D warnings` exits 0): (1) the dead `StartedServer.live` field is deleted — zero
readers anywhere, the F05 hub reaches the dispatch through `DispatchOpts.live` and the
pins construct their own `LiveState`; (2) the e2e-seam `StartedServer.brownout` field
keeps its lib-side read via `cmd_serve`'s startup diagnostic (the F12 doc's ops posture —
the governor's level rides the ops endpoints as `x-oxide-brownout`), matching the
established `started.generations` read pattern in the SIGHUP promote path; (3) the
`type_complexity` tuple in `build_dispatch_swappable` is factored into the
`RoutedGenerationHandles` alias; (4) the `redundant_closure` in the `DispatchOpts`
construction is `.map(std::sync::Arc::clone)`; (5) `brownout_for_test` builds
`BrownoutConfig` with struct-update syntax; (6) `dechunk` is a `while let` loop; (7) the
unused `futures::StreamExt` import in the resume-tee pin is gone. **The
`is_secret_key` redaction gap** (SPEC-24/§20): `log_request` guards the request log, but
`OxideRequest`'s derived `Debug` and `OxideResponse`'s manual `.field("headers", …)`
serialized RAW headers — any `{:?}` of a request carrying `cookie`/`authorization`, or of
a response carrying `set-cookie` (session tokens), leaked the value into logs, panics and
diagnostics (the redaction_tdd pin's failure message demonstrated it). Both Debug impls
now render through the shared `RedactedHeaders` wrapper (the same `is_secret_key` class;
the stable `field_with`-closure API is feature-gated `debug_closure_helpers`, hence the
wrapper type); 3 pins in `crates/server/tests/redaction_tdd.rs` (red-first, run locally —
the server crate is V8-free). **F09 CI enablement:** the `search`-feature pins never
compiled under the default-feature `rust` job (`#![cfg(feature = "search")]`); the new
additive `search-feature` CI job runs the V8-free server endpoint pins (9, verified green
locally with the feature) plus the two CLI pins by name (the serve wire-up + the
dev-refresh swap; V8-linked, CI-executed). **E03 codemod un-ignore:** verified already
satisfied in the tree — the only `#[ignore]` in the migrate tests is the deliberate
`regen_goldens` golden-regen utility (`migrate_tdd.rs` runs 24 un-ignored pins, the CLI
§19-B tests are un-ignored); no codemod test is ignored, nothing to un-ignore. The
phase-1 selfcheck now carries an explicit `EXEMPT` list for the four items this pass
deliberately edited (`StartedServer`, `build_dispatch_swappable`,
`start_server_inner_full`, `cmd_serve`) — presence and single-copy still asserted for
them, byte-equality-to-HEAD waived, the other 236 moved items still verbatim.

## V2-B05/B08 CLI integration — deltas (2026-11-16)

**B05 `KeyStr` in `L0Cache::key` (closed this diff).** The CLI's L0 key
builders (`key`/`key_for`/the test `key_of`) adopt `oxide_fw_cache::key_of`
— the 32-hex-char blake3 prefix the cache crate's `KeyStr` carries — instead
of the 64-char `CacheKey::canonical_parts` spelling. Put and get sides share
the one builder, so no mixed-format cache can arise at runtime; a stale
`dist/server/l0-seed.json` from a pre-B05 build holds old-format keys and is
cleared by a rebuild or `oxide cache clear` (the L0 tier is boot-seeded from
that manifest, never persisted independently). The deviation row's "keys
change length" note is this entry.

**B08 `NonceSplicer` at the CLI splice sites (closed this diff).** The
prerender pass's four whole-document `str::replace` calls (three
`html.replace(&nonce, NONCE_SENTINEL)` seed splices + the reuse path's
`saved.replace(&old_nonce, &nonce)`) go through
`oxide_fw_render::splice::NonceSplicer` driven single-shot
(`build/splice_nonce.rs`). The needle is ASCII hex and the replacement is the
ASCII sentinel, so a match can never split a multi-byte sequence and the
output is always valid UTF-8. The B08 dhat <1.1× allocation acceptance rides
the CLI CI job (V8-linked; the three equivalence/boundary pins compile-verify
here and run under the `nextest` workspace job).

**B01/B02 remain deliberately queued.** `RouteTree::match_ref` /
`ArchivedTree::match_ref` return structurally different views (`&RouteEntry`
vs archive index), so the dispatch switch needs a unified match view over
`RequestCtx.matched` plus a `RouteContextTable` built in
`start_server_inner_full`. The phase-3 `DispatchDeps`/`RequestCtx`/stage seam
this diff landed makes that a contained follow-up; deferring keeps this
milestone's hot-path diff purely structural (proven verbatim by
`split_selfcheck.py --stages`).

## V2-F04 stage wiring + V2-F02 build adoption — the CLI halves (2026-11-16)

The F04 substrate's recorded "to unclose" list and the F02 row's
"remaining thin halves" landed as the M1/M2 milestones:

1. **`stage_experiments` is wired** (`crates/cli/src/dispatch/stages/
   experiments.rs`, registered between `stage_middleware` and
   `stage_data_route` — inside the spec's "between prelude and the L0
   lookup" span so the vary dimension lands before the key is consumed
   and the assignments land before the loaders/data route read the
   locals). The stage short-circuits when no `experiments` config row
   exists (keys stay the pre-F04 bytes); otherwise it composes the
   substrate's `resolve` with the request probes (cookie header, header
   lookup, peer IP, the SEC-09 CSPRNG mint) and makes four write-backs:
   the `|exp:` vary dimension + the recomputed single-flight key (the
   same `key_for` spelling `stage_route` used), the assigned map into
   the pipeline-owned locals slot the loaders read, the minted
   `Set-Cookie` headers onto the per-request middleware-header list
   (applied on the L0-hit/coalesced paths AND every render response,
   never stored — the store refuses a cookie-setting response and
   `make_entry` now strips `set-cookie` as belt-and-braces, the §V2-F04
   item-3 spec text), and one `oxide_experiment_assignment{experiment,
   variant}` exposure counter per assignment. `DispatchDeps.experiments`
   carries the config rows converted at boot (`experiments_of_config` —
   the field-by-field wiring mapping; the config crate still does not
   depend on dispatch); the test helper builds the engine shapes
   directly via `DispatchOpts.experiments`.

2. **`oxide why --route` prints the experiment dimension** (§V2-F04
   item 4): for each configured experiment eligible on the route path
   it prints the sticky strategy and variants plus the `|exp:` key
   dimension shape the L0 key carries when a request lands in a cohort.

3. **The exposure metric** is the server crate's `Metrics.observe_
   experiment` (a per-pair counter map beside the vitals map; the
   exposition renders sorted `oxide_experiment_assignment` lines with
   the SEC-07 escaping). Pinned V8-free (`experiment_assignments_
   render_per_pair_counters`) and green in this sandbox.

4. **F02's build half landed** (`crates/cli/src/build/auto_islands.rs`,
   the one-line call-site recorded as PHASE_SUBS line 211): when
   `framework.autoIslands` is on, the build scans the client tree (the
   routes-dir client sources — `*.server.*`/`.d.ts` excluded — plus the
   registered islands), classifies through `scan_interactivity` + the
   import fixpoint, writes `.oxide/island-manifest.json` and feeds
   `generate_island_boot_entry` the merged registry (explicit wins; the
   engine's `hydrates` decision decides every scanned file; pure
   classifications skip). Gate off (the default) returns the config
   registry unchanged and writes nothing. Three pins ride the CLI crate
   (gate-off identity, gate-on scan/merge + side-car parse, the scan-set
   filter).

**Deliberately queued:** the F02 dev-time warn when a pure-classified
component receives `on*` props — it needs the dev transform pipeline's
per-component props surface, a separate adoption from the build scan;
the row keeps naming it. C09 (the ImageJobs endpoint wiring) and F11
(the `oxide deploy` command) were scoped this session and NOT started —
their rows still name the remaining halves honestly (the batch rule:
ship only fully-wired items).

**Pipeline pins (V8-linked, compile-verified here, riding the CI
`cargo nextest run --workspace` job):** `experiment_cohorts_get_
separate_l0_entries_and_exposure_counters` (two cohorts miss into
separate keys, the third request hits its own entry, exactly two
renders, both exposure counters counted with the engine-computed
variants), `minted_experiment_cookie_rides_the_response_and_never_the_
cache` (mint on the first response, stable identity on the second,
`make_entry` strips `set-cookie`), `experiment_assignments_reach_the_
loader_locals` (every loader run sees the pipeline-owned `experiments`
slot with the assigned variant) — plus the stage's unit pins
(`vary_with_experiments` byte shapes, the config conversion).

**Micro-debt retired along the way:** the six recorded CLI clippy
residuals (the `empty_line_after_doc_comments` in `tests.rs`, the
`unwrap_or_default` simplification in `stage_route`'s not-found
fallback — recorded as an orig-side STAGE_RULES substitution so the
span stays token-checked, two `needless_return`s in `stage_middleware`,
the `doc_lazy_continuation` in `stage_loaders`' doc header), plus the
pre-existing lints the touched-crate gates surfaced (`needless_borrow`
in `server/src/live.rs`, `manual_contains` in the server's
`rustfn_dispatch_tdd` fake, `map_unwrap_or` in `dispatch`'s
`loads_tdd` probe). `split_selfcheck.py --phases/--stages` are green
at HEAD with both recorded substitutions.

## V2-C09 serve half, V2-F11 CLI half, V2-F09 boot-flag wire, V2-E02 client half, V2-E04 client TT half (this diff)

- **C09**: the server keeps its own `ImageJobRunner` trait (boxed-job seam) so `oxide-fw-server` gains no compile-time tie to `oxide-fw-assets`; the CLI adapts the landed `ImageJobs` to it. The engine's CAS stores bytes only, so two transforms stay on the legacy direct path: a placeholder (LQIP) request (the header needs the transform's metadata) and a format-less one (the content type is only known after the decode). The ETag under the engine rides the endpoint's key (the same digest inputs the backend cache key hashes), not the backend's own cache key — both are content-addressed. `DirImageCas` lives in `crates/assets` (blake3-hex filenames; the traversal guard is the alphabet).
- **F11**: the `deploy-s3` CLI feature mirrors the `search` gate (dev builds stay lean; the target refuses loudly without it). The `Arc<T: DeployStore + ?Sized>` delegation impl follows the `ImageCas` accepted-generations pattern. `--prune`/`--rollback` are mutually exclusive usage-level refusals.
- **F09**: the flag rides a SEPARATE script (not a `hydration_bootstrap` variant) so the pinned bootstrap shapes stay byte-identical; the guard form (`window.__oxide&&(...)`) keeps it total when the bootstrap did not run.
- **E02**: the generated `href` overloads bind to the runtime `hrefFor` by shape (the route patterns, params fill, opts), not by a generated runtime — the overload's `opts` search/hash/local keys are the opts discriminator (an object with only known opts keys is opts; anything else is params; empty is params). The locale prefix convention is `/{locale}` (the `prerendered/{locale}/…` shape), skipped when the locale equals `opts.defaultLocale`.
- **E04**: the swapper routes through the boot-registered handle instead of creating its own policy (the first cut created a `default` policy inline and blew the recorded ~400-byte inline budget at 401 bytes; the handle route lands at 388). The lint test enforces the spec's `rg 'innerHTML'` rule precisely: every innerHTML ASSIGNMENT line must route through a policy path; plain reads are exempt (Trusted Types governs sinks, not reads).
- **Recorded pending**: `split_selfcheck.py --stages` runs red until the `STAGE_RULES` entry for the render-stage F09 insertion lands (the token-stream delta is exactly the flag-script block + the two `{search_flag_tag}` format! changes; `--phases` stays green).

## V2-F01 client halves + the vendor split — the island twins, the `island:render` op, the advancedChunks mapping (`feat/island-twins-island-render`, 2026-11-16)

**What lands.** Every remaining §V2-F01 client half the row names:

1. **The `<Island>` twins** — `packages/vue/src/island.ts`, `packages/svelte/src/Island.svelte`, `packages/solid/src/island.ts`, mirroring `packages/react/src/island.ts` and upgrading React's own with the `framework` prop. Local islands emit the render-crate `<ox-island data-ox-island data-ox-chunk data-ox-strategy data-ox-props>` shape; a FOREIGN island (the `framework` prop naming another framework) emits the self-contained pending placeholder — `data-ox-fw` + `data-ox-pending='1'`, no chunk/strategy, children ignored (a foreign framework's children cannot be SSR'd here). Each twin pins via its REAL framework SSR (`react-dom/server`, `@vue/server-renderer`, `svelte/compiler` + `svelte/server`, `@solidjs/web`): 5+5+5+6 pins.
2. **`resolveForeign` accepts both quote styles** — the render crate's element is single-quoted; the framework SSRs (React 19, Vue 3, Svelte 5, Solid) all emit DOUBLE quotes, so the pending-flag regex and the attribute getter match either (the capture group shift pinned). The old single-quote pins stay byte-identical; the double-quote shapes ride the same split-at-every-byte-offset property test.
3. **The worker `island:render` op** — `globalThis.__oxide_island_renderers[fw]` is the registration surface (an adapter module populates it on import); the op dispatches `(name, propsB64)` to the named framework's renderer and answers `{html}`, refusing an unregistered framework or a throwing renderer with `JSH-ISLAND` naming the framework/island. 5 pins through the real framed protocol (the `replay.test.mjs` harness).
4. **The rolldown `advancedChunks` per-framework vendor split** — the pure groups (`IslandBootPlan::vendor_groups`, 4 pins) derive from the plan's adapter set; the bundler maps them onto rolldown 1.2.15's `CodeSplittingMode::Advanced(ManualCodeSplittingOptions)` (the `MatchGroup` list, `HybridRegex` tests); the CLI build derives the groups from the same merged registry the boot entry consumes (`plan.rs`, the same `island_framework` inference). A SINGLE-framework plan emits NO groups — existing builds keep their byte-identical chunk shape (the F01 contract) — and the SSR one-file contract is untouched (`inline_dynamic_imports` wins). `hash_bundle_options` covers the groups so incremental rebuilds invalidate correctly.

**Deltas.**

- The vendor groups apply to the BUILD client bundle only; the dev bundler path (`dev/client_bundle.rs`) still uses the automatic shape (the dev chunk-name contract is load-bearing for the ESM dev server's import graph; adopting the split there is a follow-up).
- The BUILD-path mixed boot entry (the compiler planner's registry shape) is NOT wired into `generate_island_boot_entry` — the build still emits the single-adapter entry shape. The vendor split, the twins and the client's mixed registry are in place; wiring the mixed boot entry through the CLI is the remaining F01 half after this diff (recorded in the row). *(CLOSED since — the build-path wiring landed 2026-11-16, see "V2-F01 build path" below.)*
- `resolveForeign`'s new group 2 (the quote capture) shifted the attribute sides to groups 1/3 — internal, pinned.

**Verification.** 21 new TS pins (5 react + 5 vue + 5 svelte + 6 solid) all red→green locally through the real framework SSRs (the scratch node_modules: the four frameworks installed no-save at the workspace root, `@oxide-fw/core` bridged to `packages/core/src` — machine-local, gitignored); 7→11 `foreign-islands` pins; 16 worker pins (5 island-render + 6 hmr-refresh + 5 replay) green; the 4 `islands_tdd` vendor pins green.

## V2-E01 Vue/Solid refresh wiring — the compile-step emission, the runtime shim, the boot gate (same diff, 2026-11-16)

**What lands.** The remaining E01 half (the row's "the Vue/Solid refresh wiring remains"):

1. **The Vue compile step emits `__VUE_HMR_RUNTIME__.reload(id, comp)`** (spec-verbatim): the worker's `tooling:vue-compile` op appends the boundary — `import { __VUE_HMR_RUNTIME__ } from 'vue'`, an `import.meta.hot.accept((mod) => ... reload(shortId, mod.default ?? mod))` and the in-place mark. The id is the compile step's own stable short id (base64 of the path).
2. **The Solid compile step emits the refresh boundary**: `import.meta.hot.accept((mod) => globalThis.__oxide_solid_refresh?.(mod))` — the runtime seam, not a hard `solid-refresh` import (a Solid project without the package installed must not get a broken module import).
3. **The dev-only gate**: the emission rides the `hmr: true` payload flag — `esm_dev.rs` passes it, the BUILD's precompile passes nothing, so a production bundle never carries the boundary (byte-identical builds). SSR (`generate: 'ssr'`) never emits. A module already carrying `import.meta.hot` gets no double boundary (the `inject_hmr_boundary` no-op convention — with the boundary present the dev server's generic self-accept is skipped: one definition site).
4. **The runtime half** (`packages/core/src/client/hot.ts`): `__oxide_solid_refresh(mod)` — with `$SolidRefreshRuntime$` present (the probe-first prebundle pull of `/@prebundle/solid-refresh-runtime.js`, mirroring the React Refresh runtime's probe; ESM-dev-only, worker-skipped) it delegates to `performSolidRefresh(mod)`; without a runtime it does NOTHING — the boot replay applies the edit (unlike React's fallback it must NOT `location.reload()`, the replay already applied the edit). Plus the bounded in-place registry: `markInPlaceRefresh(url)` / `wasRefreshedInPlace(url)` (§3.8a bounds).
5. **The boot gate**: `bootWithRouter`'s `__oxide_on_module_updated` skips the route replay for a URL marked refreshed-in-place — the same exclusion the `refresh` capability gets for React, now per-URL for the component frameworks (a replayed `update` would remount the route and destroy the state the framework refresh preserved).

**Verification.** 6 worker pins (vue hmr/no-flag/ssr, solid hmr/no-flag/ssr) red→green through the real `@vue/compiler-sfc` + babel-preset-solid toolchain; 6 core TS pins (shim install, the runtime delegation, the throw guard, the no-reload fallback, the registry round-trip, the §3.8a bound) red→green; the boot gate wired (the `hmr-refresh` registry pins + the pre-existing boot suite keep the contract).

## V2-F01 build path — the mixed boot entry through the planner + the build-time islands.json (`feat/f01-build-path-boot-entry`, 2026-11-16)

**What lands.** The F01 row's remaining half:

1. **The CLI generator is the planner's seam.** `crates/cli/src/boot_entry.rs`'s `generate_island_boot_entry` delegates to
   `oxide_fw_compiler::islands::plan_island_boot(...).boot_entry_source(...)`; the legacy emission body is DELETED (superseded).
   The planner's single-framework branch was reordered to the legacy byte order (`installBootRuntime()` after the §8.4 loading
   map, before `window.__oxideRegisterAdapter`) so the single-framework build output is BYTE-IDENTICAL to the pre-wiring
   generator — pinned by two byte-exact fixtures (`single_framework_boot_entry_is_byte_identical_to_the_legacy_emission`,
   `the_legacy_emission_with_loading_and_pwa_is_byte_identical`, mechanically transcribed by `scripts/legacy_boot_fixture.py`)
   and mirrored by the CLI seam pins (`build_boot_entry_delegates_to_the_compiler_planner` asserts the generator IS the planner's
   emission for the same resolved set; `single_framework_build_entry_keeps_the_legacy_shape_through_the_seam`,
   `mixed_registry_build_entry_is_the_registry_shape`).
2. **The registry resolution is ONE definition site**: `boot_entry.rs::resolve_islands` (the inference order over the merged
   registry, the best-effort first-line read). The dev path's `islands_json_source` moved into `boot_entry.rs` and its inline
   copy in `dev/client_bundle.rs` deleted. The build path now writes `.oxide/islands.json` beside the boot entry
   (`boot_entry.rs::write_islands_json`, one call-site line recorded as PHASE_SUBS 218) — the spec's item 2 obligation the
   build path never carried (the dev path did).
3. **The vendor-chunk invocation**: the mixed boot entry's lazy adapter imports (`() => import('@oxide-fw/<fw>')`) are what
   pull the rolldown `advancedChunks` per-framework vendor chunks at build — the plan's `vendor_groups` derive from the same
   merged registry (the twins' mapping, unchanged). The foreign island SSR resolution rides the worker `island:render` op +
   the client's mixed registry at RUNTIME (prerendered foreign placeholders hydrate client-side — the recorded Degraded-on-edge
   shape); the build-path prerender splice of foreign islands remains NOT built (it is not part of the F01 row's half).

**Deltas.**
- The `route_ctx`-style double resolution inside the build's `phase_plan` (the vendor block resolves inline; the boot entry
  resolves through the helper) was CLOSED by the M1 micro-debt re-point (2026-11-16): the vendor block now resolves through
  `boot_entry::resolve_islands` (the ONE definition site) and the inline resolution loop is deleted — no duplicate
  `island_framework` inference remains in the phase body. The recorded PHASE_SUBS 472 replacement carries the re-pointed
  block (the token-stream delta is the deleted loop + the updated comment); `--phases` stays green. The project-framework
  derivation (`vendor_framework`) stays a local binding — the boot block's `framework` is scoped inside `boot_chunk_url`'s
  else branch, and hoisting it would rewrite a recorded region for no behavioral change.
- The planner's mixed branch keeps its own statement order (the registry `installBootRuntime({ adapters, islands })` before the
  loading map) — the mixed shape has no byte-identity contract; the order pins (`the_loading_map_contract_survives_the_mixed_shape`) hold.

**Verification.** 14 `islands_tdd` pins green (11 pre-existing + 3 new byte/order pins — red first: the pre-reorder planner failed
both fixtures); 3 CLI pins (compile-verified; the cli test target is V8-linked and rides CI's nextest workspace job);
`scripts/split_selfcheck.py --phases/--stages` green with the recorded subs (472 refreshed twice for the twins' cutoff residuals:
the needless-borrow `&src` fix and the vendor_framework block; 218 added for the islands.json write).

## V2-M2 perf tails — B03 hoists, B02 prerender wraps, A02 helper dedup, B04 resolution (`feat/hot-path-hoists`, `feat/prerender-raw-id-wraps`, 2026-11-16)

**B03** (the row names each item):
1. **Endpoint inputs** — the A01 split already carries the lazy shape (`cx.endpoint_params_json: Option` filled by
   `get_or_insert_with` in the endpoint stage; the body rides `bytes::Bytes` whose clone is a refcount bump). The remaining
   tail landed: `action_params` is built only when an action is eligible (`if action_name.is_some()` in the route stage,
   recorded as a STAGE_RULES substitution) — the spec's "built for every request though only POST actions use it".
2. **Metrics label** — the B01 table adoption already made the matched arm read `rc.pattern`; the `pattern_of` fallback is the
   P-06 degenerate arm for a table-less tree (recorded as the intended fallback, not debt).
3. **Nonce once** — the render stage's two recompute sites (`nonce_for(&req_owned.request_id)` at the streamed head and the L0
   store) reuse `cx.request_nonce` (recorded as a STAGE_RULES substitution covering both occurrences). The spec's `OnceCell`
   sketch was written against the pre-split monolith; the ctx-carried `request_nonce: String` field the route stage fills once
   is the split-idiomatic equivalent, and the acceptance `rg "nonce_for" crates/cli/src/dispatch` shows ONE definition site.
   Pin: `prerendered_nonce_is_stable_within_a_page` (a real build; one distinct nonce per prerendered page).
4. **Metrics without a write lock** — `Metrics::register_routes` fills a frozen `OnceLock<HashMap<Arc<str>, RouteMetrics>>`
   before the server starts (`start_server_inner_full` feeds every non-layout/middleware/error/loading entry's
   `default_pattern_of` — the same spelling the dispatch table builds with); `observe` answers a registered label from the
   frozen map with no lock, and `render` unions the frozen labels (sorted, zero-count histograms included) with the fallback
   map's dynamic labels. The spec-named `observe_registered_route_takes_no_lock` pin holds the fallback's write lock in
   another thread and observes through the frozen map — green in `oxide-fw-server` (V8-free).

**B02** — the build-time prerender scans adopt the table's raw-id rule: the layout chain's and the error boundary's wrap checks
prefix-match `e.id.0` (the RAW id) instead of the canonical id, exactly the `prefixes()` walk `RouteContextTable::context_for`
uses (the matcher-oracle). The resolved divergence: a group's own index page — canonical id `routes/(g)` EQUALS the layout base
`routes/(g)` — failed `starts_with("{base}/")` and missed its group layout/error boundary at prerender; the table found it. The
scan answers are oracle-equal to the table by construction. The A01 Step-0 build digests change for exactly those group routes
(the digest re-measure rides CI). Recorded as PHASE_SUBS 1133/1182.

**A02** — the CLI's duplicate `parse_form_urlencoded` / `action_from_query` copies (serve/stream.rs) are DELETED; the two stage
call sites (route.rs, action.rs) call `oxide_fw_dispatch::url::{action_from_query, parse_form_urlencoded}` (recorded as
STAGE_RULES substitutions). The copies were byte-for-byte semantics-equal (visibility/doc-only diffs, verified by diff before
deletion); the dispatch copies are the tested `pub` `#[must_use]` ones. This closes the deviation's "To unclose" item 1 — the
closure itself flipped to the stage protocol when A01 phase 3 landed.

**B04** — resolved by the A01 phase-3 split, not by the spec's `Arc<OxideRequest>` shape: the closure moves the request into the
ctx ONCE (no per-request clone exists), `OxideRequest.headers` is already `Arc<Vec<(String, String)>>`, and the only remaining
`req.clone()` in `crates/cli/src/dispatch` is prelude.rs's §13.5 proxy-forwarding branch — the spec's own stated exception. The
spec's Arc sketch targeted the pre-split closure clone that no longer exists; recorded as superseded-by-ownership (no code change).

**Verification.** `observe_registered_route_takes_no_lock` red→green in `oxide-fw-server`; `--stages`/`--phases`/full
`split_selfcheck.py` green with the five new recorded substitutions; the touched-crate battery (compiler, bundler, server,
dispatch, router, cache) check+tests green; the CLI compile-verifies via `cargo check -p oxide-fw-cli --all-targets` (V8-linked,
rides CI) with the nonce-stability pin riding CI's nextest workspace job.

## V2-M1 micro-debt re-point, V2-C09 serve-acceptance, V2-F11 deploy gate + acceptance, V2-R01 drafts (this diff, 2026-11-16)

**M1 — the vendor block's `resolve_islands` re-point.** The recorded micro-debt item (the F01-build-path entry above) is
closed: `crates/cli/src/build/plan.rs`'s vendor-group block resolves the merged registry through
`boot_entry::resolve_islands` — the ONE definition site — and the inline `island_framework` loop is DELETED. No duplicate
resolution logic remains in the phase body. The recorded PHASE_SUBS 472 replacement carries the re-pointed block (the
token-stream delta is the deleted loop + the updated comment; `--phases` stays green, 2376 lines). The project-framework
derivation (`vendor_framework`) stays a local binding: the boot block's `framework` is scoped inside `boot_chunk_url`'s
else branch, and hoisting it would rewrite a recorded region for zero behavioral change.

**C09 — the SEC-08 disable-arm pairing + the wire acceptance.** The serve wire's one landing gap: the CLI adapter maps
`OXIDE_IMAGE_UNSIGNED` to `Some("")`, but `serve_image_endpoint` required a signature for ANY `Some(secret)` — the
documented "operator explicitly disables signing" arm made the endpoint STRICTER (a guaranteed-403 dead end). Fixed at
the ONE definition site: the server's check treats an empty secret exactly like `None`
(`signing_secret.as_deref().filter(|s| !s.is_empty())`); the adapter needs no change. Pins: the red-first
`an_empty_signing_secret_answers_unsigned_requests` (server) and
`the_unsigned_env_disables_the_image_endpoint_signing` (CLI — the mapping + the default build-secret shape). The wire
acceptance is `tests/image_acceptance.sh`, CI-executed in the jshost-e2e job after the skew step's CLI build: 64
concurrent requests for ONE variant on a cold cache (`images.maxConcurrency: 64` — the shed ceiling matches the load so
the single-flight, not the shed, answers the burst), asserting every response 200 with byte-identical bodies and the
`DirImageCas` holds EXACTLY one entry — the encode counted at the store. DELTA vs the spec text: the load driver is
node's fetch at `-c 64` (`oha` is not installed on the runner and the acceptance's substance is the concurrency shape +
the one-encode count, which the CAS entry proves without log parsing); the spec's `oha -c 64` name is the shape, and
`benchmarks/load.sh`'s oha-with-fallback convention (F19/R03) still applies to the load-bench scripts.

**F11 — the parked-pin gate + the wire acceptance.** The ten object-store `DeployStore` pins are
`#![cfg(feature = "s3")]` and the default-feature CI jobs never compiled them — the PARKED-TEST rule calls that a
deferral for in-scope code, so the additive `deploy-s3-feature` CI job (the `search-feature` pattern: checkout, nasm,
toolchain, rust-cache) enables the gate: `cargo test -p oxide-fw-adapters --features s3` (the network-free
`file://`/`mem://` pins), `cargo check -p oxide-fw-adapters --features s3 --examples` (the two `s3`-gated walkthrough
examples), `cargo check -p oxide-fw-cli --features deploy-s3 --all-targets` (the CLI's `s3://`/`r2://` arm behind the
release-build feature). The wire acceptance is `tests/deploy_acceptance.sh`, CI-executed in the jshost-e2e job: the REAL
`oxide deploy` over the delta engine + the `DirStore` target — the full deploy's `bytes up` ≈ the dist (a 1 MB pad asset
dominates), the second deploy after a one-line page change uploads under 5% of the first deploy's bytes (the spec's
acceptance, parsed from the `deploy ok — … bytes up` line), an unchanged re-run uploads 0, `--rollback` swaps `current`
back to the previous build id, and an unstamped dist refuses `DEP-BUILDID` (exit non-zero).

**R01 — the conformance drafts.** The §22 suite gained one assertion per newly wired V2 feature whose surface is the
client runtime: E02 (`parseSearch` through a minimal Standard Schema V1 stub — validate, the default's `corrected`
redirect, the `OX-SRC-001` refusal with the schema issues verbatim), F01 (the mixed registry `hydrateMixedIslands` over
the adapter's REAL island SSR markup + a registry-unknown ghost — the named framework hydrates, the unknown marks
`data-ox-hydrated: error`, never a wrong adapter's hydration), E04 (the `oxide` TT policy's identity fallback + the
realm-wide `__oxide_tt` handle the deferred swapper reads), F09 (the static matcher's server arithmetic verbatim —
postings hits, the title-prefix arm, the score sort with the doc-order tiebreak, the limit take, and the post-take tag
filter the row pins as a parity quirk). 25 → 29 assertions × four adapters, ALL GREEN locally (the four real framework
SSRs; the run table prints 29 rows). C09 and F11 are server/CLI surfaces — their fixture assertions are the two
acceptance scripts above. E01's in-place registry lives behind `createHotContext` (not a per-adapter import) — its
recorded core/worker suites remain its behavior pins. Suite-level typecheck: the additions carry zero `tsc` findings in
`conformance/suite.ts` under the conformance tsconfig.

## The v0.2 Release Wave — records (2026-11-16)

**The two committed `@oxide-fw/core` build-blockers (fixed, not suppressed).** A clean-checkout verification of R01
(`pnpm install --frozen-lockfile` + `pnpm -C packages/core build`) failed at pristine HEAD twice: `tsdown.config.mjs`'s
`"search": "./src/client/searchEngine.ts"` entry carried no trailing comma (the file does not parse — every fresh core
build, the publish flow's `pnpm -r build` and the conformance gate died on it; the prior sessions' green runs rode a
dist built before the entry landed), and `boot.ts` imported `type MixedIslandRegistry` from `./odf-decode.ts` where the
type has never lived (it is `islands.ts` line 241) — rolldown's tsgo dts pass refused with `MISSING_EXPORT`. The fixes
are the one-character comma and the corrected type import (no cycle: `boot.ts` already imports `islands.ts`; islands
imports only `odf-decode`/`replay`). Neither had a test to fail — the red was the build itself; the honest pin is the
fresh-checkout recipe now recorded in the R01 row.

**R03 — the criterion gate's threshold is 10, not the default 5.** The Wave B deviations entry deferred the gate
wiring as "the R03 step"; this wave wires it into the `bench-gate` job (after the smoke-table gate and the prose
check): `cargo bench -p oxide-fw-router -p oxide-fw-cache --features bench-util` → `criterion_summary.py` →
`criterion_compare.py benchmarks/after-wave-b.json /tmp/criterion-fresh.json 10`. The threshold delta is deliberate:
criterion medians on a shared macOS runner carry more noise than the deterministic `run.sh` columns, and the gate's
proven catch (a deliberate 2× slowdown) is twenty times past 10%. Post-Wave-B bench groups (`idem`, `migrate`, `pwa`,
`sri`, `workspace_links`, …) are reported as missing and never gate until their own baseline JSON lands —
`after-wave-b.json` covers the Wave B ids only, and the report's criterion section now carries their session-recorded
numbers with that provenance stated. The G2 targets that cannot run in this sandbox (the 3× native cold build, the
<200 ms fan-out, the CLI-side ≤12-alloc L0-hit dhat harness) are marked CI-pending in `benchmarks/report.md`'s G2
table with their exact capture commands — deferral of EXECUTION only, per the standing rule.

**R05 — the review lives at `docs/security-review-v0.2.md`.** The spec's R05 names `docs/security/v0.2-review.md`;
the flat `docs/` placement is the recorded delta (the repo has no `docs/security/` directory and every other review
artifact — the scorecard, the release audit — sits flat in `docs/`). The review's five gaps are recorded, not
silenced: the stream↔session-cookie-hash binding, the `odf_patch`/`rustfn_shim`/`stream_range` fuzz targets, the
search query-length constant, the scaffold gitignore verification, and the prototype-pollution fuzz target. The
`__proto__`/`constructor` rejection itself IS implemented (`packages/core/src/client/odf-decode.ts` — keys rejected
anywhere in a patch, segments rejected in paths) and pinned; the fuzz target is the remaining hardening, not the
control.

**R06 — the two version-cadence drift facts.** The crates + root `package.json` move together (the
`chore(release): v0.17.0` commit touched `Cargo.toml`, `Cargo.lock`, `package.json` and nothing else); the seven
publishable npm packages move in their own lockstep (0.6.x). Consequences recorded in `docs/release-v0.2-prep.md`:
`scripts/bump-version.sh`'s "every surface at the same version" precondition refuses while the cadences differ (use
the manual commands, or reset the script in a separate chore), and `packages/tailwind` at 0.6.0 vs its siblings at
0.6.1 means `publish-npm.yml`'s version gate cannot pass for ANY input until tailwind is aligned. The spec's
`contractMinor: 1` field (V2.2.5) exists nowhere in `packages/*/src` at HEAD — land it in the release PR or record
its deferral before tagging; it is additive-optional and does not block binary compatibility.

**R04 — the conformance count drift.** `docs/architecture.md`'s testing section still said "24 assertions × 4
adapters"; it now says 29 with the C09/F11 acceptance-script routing (the E01 in-place-registry note included). The
README badge was re-measured from a clean install with every package dist built: **2531 Rust** (the static count,
verified against `count-tests.sh`) is unchanged — the wave adds no tests — but the TS half moved **754/757 → 760/760**:
the prior measurement predates this session's full-suite runs, and this session ran every client suite from a fresh
`pnpm install` (react 90/90, vue 67/67, svelte 67/67, solid 63/63, create-oxide 19/19, core 422/422,
conformance 32/32 — 760 pass of 760, zero failures; the three vue/svelte reds seen before the adapter dists were
built are the export-map pins asserting `dist/`, gitignored on a fresh checkout and built by CI before the suites).
Known script drift recorded: `count-tests.sh --badge`'s TS half greps the TAP `# pass` summary, which node 24's piped
reporter does not emit (it prints the spec-reporter `ℹ pass` shape) — the badge's TS value was measured by summing
the suite summaries directly, and the script's TS arm needs a reporter-shaped grep to work again.

## The R05 implementation gaps close — the stream↔session binding + the search query-length cap (this diff)

**M1 — the §13.4 stream↔session binding** (`crates/server/src/stream_store.rs`
+ the CLI wire): the security table's "bind id to the original request's
session cookie hash; mismatch gives 410" is implemented as an injectable
seam, the codebase's standard shape:

- `SessionBinding` — the opaque 32-byte digest newtype; no `PartialEq`
  (every identity check rides `matches`, the crate's SEC-04
  `constant_time_eq` — promoted `pub(crate)` so the crate's secret compares
  keep ONE definition site); a redacted `Debug` (the digest IS the
  reattach credential).
- `SessionBindingSource` — the object-safe seam; the default
  `CookieBindingSource` hashes the named cookie value with blake3 KEYED by
  a per-boot random key (ring `SystemRandom`, the session codec's RNG):
  two constructed sources never agree (pinned), so a binding minted by one
  serve never validates against another's and a leaked digest cannot be
  replayed offline. An RNG failure at construction is `SEC-SESSION`
  (the registered session code — no registry row needed).
- `StreamStore::begin_bound(id, Option<SessionBinding>)` — the tee site
  mints BOUND to the ORIGINAL request's identity; `begin` keeps the
  unbound semantics (today's shapes, pinned unchanged).
  `StreamStore::get_bound(id, Option<&SessionBinding>)` — a mismatch
  (foreign OR missing cookie) is the same `Reattach::Gone` an unknown id
  gets (the endpoint's `410` is unchanged, so a refusal never confirms
  the id exists), with one loud warn per refusal that carries the stream
  id and NEVER the cookie or digest (the SEC-07 never-logged rule).
- The CLI wire: `serve/session.rs` gains `SESSION_COOKIE_NAME` (the ONE
  definition site — the `Set-Cookie` emission and the binding source spell
  the same constant); `serve/streams.rs`'s `StreamWiring` carries the
  boot-owned `CookieBindingSource` and delegates through `binding_of`
  (the stages call the wiring method — no trait import in a stage body);
  the tee arm (`dispatch/stages/endpoint.rs`) and the reattach arm
  (`dispatch/stages/prelude.rs`) adopt `begin_bound`/`get_bound`.
  Both arms are recorded as `split_selfcheck.py` STAGE_RULES entries —
  `--stages`/`--phases` green (55,496 tokens, zero diffs).

**M2 — the §13.4 search query-length cap**
(`crates/server/src/search_endpoint.rs`): the spec's "query length cap
(200)" is the named `MAX_QUERY_LENGTH` constant, enforced BYTES-first in
`search_response` — before the L1 cache and the engine ever see the text
(an oversized `q` can never become cache-key material or an engine parse).
The refusal is the endpoint's plain-text `400` and its body names the cap.
Pins: over-cap 400, the inclusive boundary (exactly 200 queries, 201
refuses), and never-reaches-the-engine. The new `search` criterion bench
measures the refusal path against the L1 hit (session numbers, short-sample
criterion: `stream_binding/hash` ≈ 93 ns — the keyed hash per reattach;
`search_gate/over_cap_refusal` ≈ 13.9 µs, dominated by the 400 response
build, not the gate).

**Micro-debt retired along the way:** `count-tests.sh`'s TS arm greps BOTH
reporter shapes now (node 24's piped reporter prints the spec-reporter
`ℹ pass N`; the recorded drift closed — the grep is `(^# pass|ℹ pass)`,
verified against a live node 24 run); the badge re-measured **2531 → 2542
Rust** (8 stream pins + 3 search pins; TS unchanged — no TS changes), the
README badge updated in place.

**Docs drift fixed:** `docs/security-review-v0.2.md`'s Section 2 gap
flipped to landed (a new threat row: reading a DISCLOSED id from another
session) and Section 4's gap flipped to landed (the oversized-query row);
the §8 honest list now names items 1 and 3 as landed with their pins, and
`docs/spec-v0.2-draft.md`'s R05 row carries the pointer.

**Deliberately not done here (still honest gaps):** the `odf_patch` /
`rustfn_shim` / `stream_range` fuzz targets (cargo-fuzz is nightly-only —
out of this sandbox's stable-only gate), the create-oxide scaffold
gitignore verification (the release checklist's per-app verification), and
the F02 dev-warn half (the V8-linked dev transform pipeline). The CLI
stream arms are compile-verified only (`cargo check -p oxide-fw-cli
--all-targets`); their runtime pins ride CI's nextest job, per the
recorded convention.

## The F02 dev-warn half + the E10 endpoints page land — the props surface, the pure-classified mismatch warn, the endpoint-map data and its generated reference page (this diff, 2026-11-17)

**M1 — the F02 dev warn** (`feat/f02-pure-onprops-dev-warn`): the
spec row's last thin half — "the dev-time warn when a pure-classified
component receives `on*` props — it needs the dev transform pipeline's
per-component props surface, a separate adoption from the build scan" —
landed:

- The engine (`crates/compiler/src/props_surface.rs`, the
  `oxide_fw_compiler::props_surface` seam): `scan_on_props` reports every
  JSX component element (uppercase-first identifier or member base)
  carrying `on*` attributes — the classifier's own 2-char
  `on`-prefix rule mirrored verbatim, so the two surfaces can never
  disagree about what `on*` means — with the props sorted, the 1-based
  line and the trimmed source line (the classifier's `enrich` shape).
  `scan_component_imports` maps the local bindings (default, named,
  aliased, namespace) onto their specifiers. One Oxc parse collects both
  (the dev pipeline pays it once per served module).
- The cross-reference: `pure_on_props_in_file` (per served module — the
  usage scan reads the CURRENT source, so a fresh edit is caught on the
  next serve) and `pure_on_props_warnings` (whole-project, the dev-boot
  shape) warn ONLY on a positive pure classification. A defining file the
  manifest classifies interactive (including a fixpoint-promoted one),
  does not know (the `hydrates` missing-data rule, mirrored), or that is
  the usage file itself (carrying the `on*` attribute makes the file
  interactive by its own scan — a same-file pure definition is a
  contradiction) never warns. Package specifiers carry no same-project
  evidence (the fixpoint's relative-only rule, shared via the extracted
  `relative_join`/`specifier_candidates` helpers in `interactivity.rs`).
- The dev wire (`crates/cli/src/build/auto_islands.rs` +
  `crates/cli/src/esm_dev.rs`): `dev_island_manifest` scans the SAME
  `scan_set` the build runs (one rule, two consumers — the extraction is
  `scanned_sources`), read-only: no side-car write, no registry merge.
  The manifest rides `EsmDevState.islands`; both ESM serve arms
  (`serve_source_file` and the pool-compiled path) check the served
  module and log `AST-ISL-002` once per usage site per dev session
  (the dedup key is `usage_file:component:line`). The gate-off state is
  a no-op; the serve path (`serve/start.rs`) populates the same state.

**Deltas (recorded, the honest limits):** the surface is JSX-shaped —
the `.vue`/`.svelte` pool arms' sources carry no JSX and warn nothing
(their templates' event syntax is already direct-detection evidence in
the classifier); the warn matches `on` + anything (the classifier's
spec-verbatim rule) rather than the event-prop convention, so a stray
`only` prop can warn — the message names every prop, visible not silent;
spreads claim nothing (`{...rest}` is not a named prop); the
classification is the dev boot's (a mid-session change to a defining
file refreshes on the next boot — the usage side is live, the
classification side is frozen); the adoption rides the native ESM dev
pipeline (`dev.esm`), not the bundler dev path (a different transform
seam); JSX resolution is specifier-level (the module the specifier
names), not symbol-level (no bundler resolution in the engine).

**M2 — the E10 endpoints page** (`feat/endpoints-reference-page`): the
E11 generator's remaining thin half — `endpoints.md` (the V2.J table) —
landed: `crates/server/src/endpoints.rs` (the `ENDPOINT_MAP` data — 12
rows verbatim — plus `endpoint_markdown_table()`, the deterministic
GFM fold; the plane rule held row by row in the tests: data/dev ride the
`/__` families, ops rides `/_oxide`) and the `EndpointsPage` generator
in `crates/cli/src/docs_gen.rs` (registered in the alphabetical
registry; the page folds the server crate's map — the capability-matrix
rule: the map is the single source of truth, the page never restates a
cell). The committed `docs/reference/endpoints.md` regenerates via
`oxide docs generate`; the `docs-check` CI job holds it.

**Registry re-sort (micro-debt with a red gate):**
`docs/errors.toml` carried six out-of-order pairs (hand-inserted rows
grouped visually — `AST-ISL-001` before `AST-IDX`, `AST-JOBS` before
`AST-IMG-REQ`, `DEP-ROOT` before `DEP-ROLLBACK`, `DEV-TLS` before
`DEV-RFN-001`, `FN-TIMEOUT` before `FN-THREW`, `RFN-TYPES` before
`RFN-002`), which made `registry_parses_and_is_wellformed`'s
sorted-by-code contract RED at pristine HEAD. The file is re-sorted
lexicographically (247 entries; the page's rows move with it — the page
renders file order). `AST-ISL-002` rides the sort (the F02 warn's code,
raised at `crates/cli/src/esm_dev.rs`).

**Docs drift fixed:** the stale "Inert at the wiring" claims in the
config crate — `deploy` (F10's serving-pool CLI adoption + F11's deploy
command landed), `offline` (the F14 wire-through landed), `record` (the
CLI record/replay adoption landed) and `framework.autoIslands` (the
build scan landed) — in `crates/config/src/lib.rs` and
`crates/config/src/schema.rs` (the generated config.md and the committed
schema JSON follow them);
`docs/architecture.md`'s object-store bullet still carrying "the
`oxide deploy` CLI command remains the F11 tail" (closed by its sibling
bullet), the E01 what-changed bullet's stray "since 2026-10-12" fragment,
and the compiler row + what-changed bullets for this diff;
README.md's "Twenty Rust crates" listing 20 names (22 exist —
`macros` and `rustfn` were missing; the CLI command list gained
`deploy`, `replay`, `fmt`, `gc`, `bisect`); the badge re-measured.

**Micro-debt:** the workspace's last `#[allow]` —
`#[allow(unused_variables)]` on `register_compiled_in`
(`crates/compiler/src/native/mod.rs`) — removed: the compensating
`#[cfg(not(any(native-*)))] let _ = reg;` dead-use already keeps the
parameter used in the all-off build, so the attribute was redundant
weight on the no-allow policy; the grep gate over `crates/` is now
EMPTY. `scan_svelte_markup`'s `unwrap_or_default()` on the blanked
markup buffer (`crates/compiler/src/interactivity.rs`) now falls back to
the ORIGINAL text — an impossible-in-practice UTF-8 failure degraded to
an EMPTY scan (every template hit silently missed: the kill-criterion
direction); the fallback over-flags instead (the conservative arm).

**Deliberately not done here (still honest gaps):** the bundler dev
path's props-surface adoption (a different transform seam — the ESM
pipeline is the one the queued note names), symbol-level import
resolution in the warn (specifier-level is the engine's scope), and the
E05-dependent `plugin-compat.md` page + the docs-site Oxide app (the
other E10/E11 tails). The CLI pins (the 3 esm-dev warn pins + the 2
dev-manifest pins + the 2 endpoints-page pins) are compile-verified
locally (`cargo check -p oxide-fw-cli --all-targets`); their runtime
rides CI's nextest workspace job, per the recorded convention.

## The WASM plugin `load`/`resolveId` ABI + the compat matrix (SPEC-15.2/15.3, E05 substrate)

**What the spec asks.** The §15.2 hook set usable from every plugin kind, with
a published compatibility table (`docs/plugin-compat.md`) that tells the truth
per kind × hook.

**What lands.** `crates/plugin/src/compat.rs`: `SupportLevel` (the four
published-table markers), `compat(kind, hook)` — the single source of truth,
reading `Hook::supported_in_bridge` for the JS column and the WASM host's
landed ABI for the WASM column — and `compat_markdown_table()`, the
deterministic render the published table embeds byte-identically (pinned by
`compat_tdd::plugin_compat_doc_embeds_the_generated_table`; the E11
`doc_table_matches_the_function` pattern). The WASM host gains the two
option-returning call paths (`call_load_hook`/`call_resolve_hook`, probed as
`(string) -> option<string>` and `(string, string) -> option<string>`) and
`missing_hooks()` (the compat runner's `unsupportedHooks`); the object-safe
[`HookHost`] seam (`lib.rs`) carries the three callable bridges so a pipeline
programs against `dyn HookHost`.

**Deltas.**
- The table's WASM `load`/`resolveId` rows flip 🚫 → 🟡 (host-callable, CLI
  call site pending) and the JS `configureServer`/`cacheHandler` rows flip
  ✅ → 🚫 (`PLG-HOOK` at load — the code and the legend always said so; the
  hand-maintained table disagreed).
- `hook_export_name(Hook::ResolveId)` flips `resolve_id` → `resolve-id`:
  the component model refuses underscore extern names, so the snake spelling
  was never exercisable. The other multi-word hook names stay snake (they have
  no call path yet; their rows remain 🚫 and the rename rides their landing).
- The WASM `transform` ABI stays `(string) -> string` (not the JS `(code, id)`):
  the host passes the source, the id rides the call-site closure — recorded
  here because the table's Notes column carries the JS shape.
- `HookSig` gained the two option shapes; `load` keeps its non-optional
  `call_string_hook` path (back-compat for `(string) -> string` exports).
- The temporary `wat` dev-dependency used to diagnose the canonical-ABI
  fixtures is gone; the landed fixtures hand-write the canonical ABI
  (return-pointer convention, `cabi_realloc` bump allocator) with inline WAT.

**Dead deps.** `wit-parser` and `wasmtime-wasi` rode the plugin crate unused
(the host never opened WASI preopens) — removed from the crate and the
workspace table (the C01 shed-the-dead-deps precedent).

**Remaining half.** The CLI wire: `build_transform_hook` (crates/cli/src/hooks.rs)
wraps only `Transform` today; the adoption composes the loaded component's
`load`/`resolveId` into `BundleOptions.load_hook`/`resolve_hook` (the bundler
consults both, `lib.rs:772`/`:852`) beside the JS bridge's hooks. V8-link
cost rides CI per the recorded convention.

## The E08 open-in-editor + F17 cost endpoints land — the §V2.J dev-plane rows go from advertised to answered (`feat/open-in-editor-endpoint` + `feat/cost-endpoint-seam`, 2026-11-17)

**What lands (M1, E08 item 1's server half).** `crates/server/src/open_editor.rs`:
`GET /__oxide/open?file=&line=` — the §V2.J map's dev-plane row the endpoint
map and the committed page already advertised, answered at last. The module
holds the editor chain (`resolve_editor`: `OXIDE_EDITOR`, then `EDITOR`, then
the documented `code -g` default — resolved PER REQUEST so a dev session can
export the variable late), the project-root confinement (`confine`:
canonicalize the joined path, require it inside the canonical root AND an
existing file; the response contract: non-GET `405` + `allow`, bad params
`400`, a confinement refusal `422` with the coded `DEV-OPEN-001`, a spawn
failure `500` with `DEV-OPEN-002`, a hit `200` JSON
`{ ok, editor, file, line }` with the root-relative spelling), the
`EditorOpener` seam (the production `CommandOpener` spawns detached with null
stdio — the dev crate's `open_browser` shape; fakes record), and the
`OpenEditorEndpoint` the pipeline arm consumes. The arm rides the native
prefix right after the inspector's (`3a.2.1`), dev-token guarded (D41) with
the same constant-time compare — the token guard runs before the method
check, because the endpoint spawns a process. Attached via
`Pipeline::with_editor_opener`; `None` (the default) leaves the endpoint
absent and production byte-identical. 22 TDD pins (red-first) + 2 module
pins + the `open_editor` criterion group + the walkthrough example; the
`DEV-OPEN-001`/`DEV-OPEN-002` registry rows ride `docs/errors.toml` and the
regenerated page.

**What lands (M2, F17's server half).** `crates/server/src/cost_endpoint.rs`:
`GET /__oxide/cost?src=<path>` — the second path of the SAME §V2.J row. The
spec-verbatim `ComponentCost` record (`bytes`/`gzip`/`chunk`/
`renderMsP50`/`cache`, camelCase on the wire; `renderMsP50` and `cache` null
for the bytes-only adapters — the overlay's "render time unavailable" note
reads the null), the injected `CostSource` seam (the endpoint is the wire
contract; the build-side adapter feeding the bundler's module map and the
render profiler is the caller's), the arm (`3a.2.2`, dev-token guarded) and
`Pipeline::with_cost_endpoint`. 9 TDD pins (red-first) + the
`cost_endpoint` criterion group + the walkthrough example. No new error
codes — the endpoint has no coded failure shape (the nav pattern: plain-text
statuses).

**Deltas.**
- The editor env value splits on WHITESPACE (glob-lite, no shell, no new
  dep): a quoted editor path with spaces is not supported. The escape hatch
  is the placeholder rule: `OXIDE_EDITOR='vim +{line} {file}'` substitutes
  every `{file}`/`{line}` occurrence; a value naming NO placeholder gets the
  target appended as one `path:line` argument (the VS Code `-g` convention
  the default spelling needs).
- Inside-root `..` spellings NORMALIZE (canonicalize-first) instead of being
  banned by a textual `..` scan: `src/../src/a.rs` opens the same file as
  `src/a.rs`; only ESCAPES refuse. The spec's "rejects `../`" pin holds on
  the escape shapes.
- The endpoint requires the target to EXIST and be a file (the overlay only
  links served sources; the editor cannot open a not-yet-written path).
- The `file` param is percent-decoded (the nav endpoint's parse shape), so
  space-bearing source names work on every platform.
- The `file`-param resolution runs from the REAL process env per request
  (not a boot-time snapshot) — the recorded testability delta is the
  resolver's `&dyn Fn(&str) -> Option<String>` parameter, which the pins and
  the example drive with fakes.

**Remaining halves (recorded, not stubbed).** E08: the overlay's
source-mapped frames + "open in editor" button, the `/__oxide` inspector and
the doctor additions. F17: the compiler's `data-ox-src` attribute injection
(Oxc visitor on JSX; the Vue/Svelte compile hooks) and the overlay UI. Both
endpoints' CLI attach is the recorded one-liner (`with_editor_opener` /
`with_cost_endpoint` at the dev boot, beside `with_inspector`) — the
V8-linked crate rides CI per the convention.

**Micro-debt (this diff, additive).**
- The dead `oxide-fw-cache` dependency in `crates/server/Cargo.toml` is
  removed (zero inheritors — the crate's sources never name it; the manifest
  advisory `cargo::unused_dependencies` printed on every build is
  silenced).
- The disk-tier migration loop's corrupt-v1-row fallback
  (`crates/cache/src/tiers.rs`) is loud now: a row that does not parse as a
  JSON key list warns `CACHE-CODEC` naming the tag and migrates as EMPTY —
  the empty-set behavior unchanged and self-healing, but the invalidation
  gap is visible instead of silent (the `CACHE-HTTP` tag-key precedent).
- The spec's F09 row still claimed the F09 render-stage `STAGE_RULES`
  substitution "pending — `split_selfcheck.py --stages` runs red until it
  lands"; the selfchecks are GREEN at HEAD (verified from a full-history
  clone this session — the shallow-clone 128 the row's claim probably
  grew from), so the stale pending sentence is retired with the verified
  fact recorded in its place.
- `docs/spec-v0.2-draft.md`'s E04 row is flipped `partial` → `merged` (the
  client TT half landed last session; the row still said partial) and its
  mid-row "Remaining" sentence is retired; the row's stale `packages/oxide`
  spelling for the TT sinks is corrected to `packages/core` (where
  `client/tt.ts` actually landed).

## The F17 mapping + measurement halves land — the overlay's DOM→source stamp and the cost endpoint's data producer (`feat/data-ox-src-injection` + `feat/source-cost-engine`, 2026-11-17)

**What lands (M1, the §V2-F17 DOM→source mapping, JSX-framework half).**
`crates/compiler/src/data_src.rs`: `inject_data_ox_src(code, lang, path)` —
in dev, the first element each top-level component renders carries
`data-ox-src="path:line"`. The rule set: a component is a top-level item the
React-Refresh rule already recognizes (function/class declarations and their
`export`-wrapped and default-export forms, arrow/function/class expressions
bound to a variable) filtered by the uppercase-first name rule the islands
classifier uses — an anonymous default export IS a component (the route
module's root), lowercase top-level helpers are not. The first element is the
first `JSXElement` lexically inside the component's first JSX-bearing
`return` (fragments transparent, conditional returns skip the arms that
render nothing, wrappers descended: await, TS casts, conditional/logical
arms, call/new arguments, array/object literals, callback bodies); elements
not in return position are never stamped. The line is the 1-based line of
the component's declaration; the path part is the caller's transform `path`
(root-relative, the `?src=` spelling). The edit is span surgery on the
ORIGINAL text (the `inject_react_refresh` emit pattern): only the attribute
bytes are inserted, every other byte — and every line count, so the dev
sourcemap stays valid — is preserved. Idempotent: an element already
carrying the attribute is skipped (the dev pipeline re-serves modules). The
no-vouch direction: a non-`tsx`/`jsx` language, an unparseable source or a
module with no component is returned unchanged. Wire-up:
`lower_to_js_with_refresh` (the dev lowering entry the ESM dev server serves
modules through — `crates/cli/src/esm_dev.rs`'s existing call site) runs the
stamp BEFORE the JSX lowering, so the runtime's own JSX pass carries the
attribute into the props call; production `lower_to_js` never runs it. 16
integration TDD pins (red-first) + 3 module pins + the `data_src` criterion
group + the walkthrough example.

**What lands (M2, the build-side producer).** `crates/bundler/src/cost.rs`:
`source_costs(output, root)` — the per-source `{ bytes, gzip, chunk }`
records the landed `/__oxide/cost` endpoint's `CostSource` seam serves. The
spec's measurement method implemented verbatim: `bytes` is the module's
carried source length, `gzip` is the REAL RFC 1952 gzip-stream length over
the same bytes (flate2, the workspace's own gzip crate — not a ratio guess),
`chunk` is the file of the FIRST chunk listing the module (`None` when no
chunk claims it). The `source` key is the root-relative spelling with
forward slashes — the same spelling the `data-ox-src` stamp carries and the
`?src=` query takes, so the overlay's fetch needs no re-spelling; a module
outside `root` rides as given. Assets and server-only modules are excluded
(the overlay attributes CLIENT sources). Sorted by the source key — two
builds over the same output produce byte-equal tables. 12 TDD pins
(red-first) + the `cost` criterion group + the walkthrough example. The
`renderMsP50`/`cache` fields are NOT computed here — they belong to the
render profiler and the cache tier map, the adapters that feed the endpoint
alongside this engine.

**Deltas.**
- The component rule is the uppercase-first NAME rule, not merely the
  refresh rule: React Refresh registers every top-level function, but the
  stamp puts overlay handles on rendered DOM — a lowercase helper's rows
  would misattribute to whatever real component lists it. The anonymous
  default export (no name to filter) still stamps.
- An arrow's implicit-return expression is searched directly
  (`ArrowFunctionBody::Expression`); a block arrow walks like a function.
  A class component renders from its method/property bodies in declaration
  order (the `render` method in React; Solid/Svelte class forms).
- The parser's `preserve_parens` default (true) is load-bearing: a
  parenthesized return argument is descended (`ParenthesizedExpression`
  arm). The pass matches the source text the parser saw, not a codegen
  round trip — the double-parse cost the alternative would pay per served
  module is avoided AND the byte-preservation property holds.
- The stamp rides `lower_to_js_with_refresh` unconditionally for tsx/jsx
  (the function IS the dev lowering entry — the refresh injection's
  existing posture), not behind a separate flag: the dev server's call
  site needed zero edits and the V8-linked crate stays on CI.
- `gzip` measures the module's carried (post-walk) source, not the minified
  chunk text: the record labels the basis honestly and the overlay compares
  sources against sources; the endpoint's `chunk` field carries the
  wire-level truth.
- The Vue/Svelte injection halves ride their compile hooks (the tooling
  pool's `@vue/compiler-sfc` / `svelte/compiler` steps), not the JSX path —
  recorded remaining, with the overlay UI.

**Micro-debt (this diff, additive).** `crates/server/src/navgraph.rs`'s
`NavGraph::persist` serialized the snapshot into the tmp file with
`serde_json::to_string(&body).unwrap_or_default()`: a serialization failure
wrote an EMPTY tmp and the atomic rename then promoted it over the previous
snapshot — the learned nav edges would vanish silently. The serialization
now runs BEFORE the tmp file exists and the error rides the function's
`io::Result` (the synthetic-tsconfig precedent; the flusher task already
logs a failed persist and keeps serving the previous snapshot).

## The E08 inspector's live + generation surfaces and the F17 adapter half land — the inspector shows the live hub and the build generations, the cost endpoint gets its production adapter (`feat/inspector-live-generations` + `feat/cost-source-snapshot`, 2026-11-17)

**What lands (M1, the §V2-E08 inspector row's two missing surfaces).**
The row lists "live subscribers (F05), generations (F10)" among the
`/__oxide` surfaces; the §13.4 model carried neither — the schema stayed
honest, the surfaces stayed empty. Three coordinated pieces:

- `crates/server/src/live.rs`: `Hub::tag_snapshot()` — per-key live counts
  over both tag indexes (exact tags + the wildcard-prefix keys, the F05
  step-4 index), sorted by key for a stable render, dead entries skipped,
  rows never fabricated (an empty hub snapshots empty). The record is
  `LiveTagCount { tag, wildcard, subscribers }`; a connection holding
  several tags contributes to each of its rows — per-tag counts, not a
  partition (the global count stays on `count()`).
- `crates/dispatch/src/build.rs`: `GenerationSet::snapshot(now)` — the
  inspector's generations rows (`GenerationInfo { id, role,
  window_ms_remaining }`): the current generation always, the demoted
  previous only while its window is open — an expired previous answers 409
  (the `accepts` verdict), so it leaves the rows. The same `now` arithmetic
  `accepts` applies, so the row and the verdict can never disagree; the
  remaining window is `checked_sub`-carried (a skewed caller clock
  saturates to the full window instead of panicking).
- `crates/dev/src/lib.rs`: the schema the CLI maps into —
  `LiveRow`/`GenerationRow` rows, `InspectorState.live`/`.generations`
  (`#[serde(default)]` — a pre-E08 payload still parses),
  `RouteTableInspector::with_live`/`with_generations` builders (the
  wire-up point the serve wiring constructs), and the two HTML tables in
  `render_inspector_html` (counts in the headings, escaped cells, the em
  dash for the current generation's absent window). The producers return
  their own plain records and the mapping is the caller's — the same
  layering as the cost endpoint's seam: server and dispatch do not depend
  on the dev crate, the CLI adapter is a field-by-field one-liner pair
  that rides CI's nextest job (the recorded V8-link convention; the serve
  wiring constructs `RouteTableInspector` at `crates/cli/src/serve/start.rs:707`).

7 TDD pins (red-first: E0432/E0422/E0560/E0599), the `tag_snapshot`
criterion group (≈5.2 µs at 256 / 19.4 µs at 1024 connections), the
`generation_snapshot` group (≈62/122 ns at 1/2 rows), the dev
`inspector` bench (`inspector_state/256` ≈ 20 µs, `inspector_html/256`
≈ 102 µs — the per-dev-console-view cost, not per request) and the
`inspector` walkthrough example. No new error codes — the read-only
snapshot APIs cannot fail (the nav-pattern rule).

**What lands (M2, the F17 adapter half).** `crates/server/src/cost_endpoint.rs`:
`SnapshotCostSource` — the snapshot-backed production `CostSource` the
endpoint doc recorded as "the caller's": the build's cost records held in
memory keyed by the root-relative spelling the `?src=` query and the
`data-ox-src` stamp share (`from_pairs` takes the bundler's
`Vec<SourceCost>` shape one-to-one), plus `ComponentCost::bytes_only` —
the spec's Svelte/Solid shape ("Svelte and Solid show bytes only, and the
overlay says so"), the constructor the bundler triple maps through with
the render and cache halves `None`. Hits answer from the build's records;
anything the build did not produce is the documented 404, never a
fabricated record. 3 TDD pins (red-first) + the `cost_source_snapshot`
criterion group (≈54 ns at 64 records, 87 ns at 4096 — the BTreeMap
lookup the endpoint pays per query) + the adapter section in the
`cost_endpoint` walkthrough example. The render-time (`renderMsP50`) and
cache-tier halves still belong to the profiler and the tier map — this
adapter closes the records-flow gap the two landed F17 halves left; the
CLI feed one-liner rides CI with the serve wiring.

**Micro-debt (additive).** The dead workspace dependencies removed —
`oxide-fw-odf` + `thiserror` out of `crates/render`, `thiserror` out of
`crates/graph`, `crates/odf` and `crates/cache` (zero inheritors, the
`cargo::unused_dependencies` advisories silenced); cache's `futures` moved
under the `s3` feature that uses it (`s3 = ["dep:object_store",
"dep:futures"]`) and render's `tokio` to `[dev-dependencies]` (both were
flagged unused for their lib targets). The bundler's silent hook writes
made loud (`crates/bundler/src/rolldown_backend.rs`): a renderChunk hook's
output that fails to write now warns naming the file (both the dev and the
build path — the hook's output is the build's product; the old
`let _ = fs::write` reported success while shipping the un-hooked source),
and the synthetic-tsconfig directory create names its own failure instead
of surfacing later as an unrelated write failure (the file's own
synthetic-tsconfig warn is the modeled shape).

**Deltas.**
- The snapshot drops dead index rows instead of rendering zeros: the
  `Guard::drop` sweep already removes a dropped subscriber's entries, so a
  zero row can only be a stale `Weak` — the honest table is the empty one.
- `GenerationSet::snapshot` takes `now` as a parameter (the `accepts`
  signature) — deterministic at a given instant, testable without sleeps.
- The inspector's HTML renders the two new tables with count headings
  even when empty (the routes/waterfalls tables' existing shape — the
  schema stays visible; no fabricated rows inside).
- The wire shapes stay snake_case like the rest of the §13.4
  `InspectorState` record (no TS consumer reads it; the raw JSON view is
  the contract).

## The E08 inspector's module-graph search and the F17 profiler + tier-map adapter land — the inspector can search the harvest, the cost record gains its remaining halves (`feat/inspector-module-search` + `feat/profiler-cost-adapters`, 2026-11-17)

**What lands (M1, the §V2-E08 inspector row's module-graph search).**
The row's item 2 names "module graph search and `why`"; the §13.4 model
carried the `module_graph` field (module → importers, the harvest's
reverse-deps shape) but nothing populated it, the HTML never rendered it,
and no search existed. Four coordinated pieces:

- `crates/dev/src/lib.rs`: `search_module_graph(graph, query)` — the
  deterministic search: a case-insensitive substring over the module path
  OR any importer path, input order preserved (the harvest feeds a sorted
  `BTreeMap` walk; the search never re-sorts, so the caller's order is
  the render's order), a blank query is every row, a miss is an empty
  vec. A matched row carries its importer edges inline — that IS the
  `why` answer (who pulls the matched module in), one lookup, no second
  table.
- `RouteTableInspector::with_module_graph` — the wire-up builder the
  caller maps the harvest's records through, one-to-one, sorted order
  carried. `state()` now populates `InspectorState.module_graph` (the
  field the schema carried since §13.4 — the wire shape is unchanged).
- The HTML: `render_inspector_html_with_query(state, query)` — the
  Modules table (module + importers, escaped cells) between Routes and
  Live, the heading keeping the total, a `k of n` note with the
  ESCAPED query echo when a non-blank `q` filters the rows, and the
  search form (`GET /__oxide?q=…`, a plain HTML form — no external
  assets, keyboard-operable) rendered ONLY when a graph is wired: a
  search over nothing would be a fabrication. `render_inspector_html`
  delegates query-less; the pre-search byte shape is unchanged apart
  from the new section.
- The server arm: `GET /__oxide?q=<query>` extracts `q`
  (`percent_decode_query`, the cost endpoint's `src=` spelling), a blank
  or foreign query keeping the unfiltered render (the pre-search byte
  shape), and passes it through the new `InspectorSnapshot::render_html_search`
  — default delegates to `render_html`, so a pre-search implementor
  keeps compiling AND keeps answering `?q=` with the same page (the
  documented degrade, not a stub). The CLI's `CliInspector` overrides it
  with the dev crate's filtered renderer, and the serve boot feeds the
  module graph from the boot-time harvest (`build_hmr_graph`'s importers
  map) beside the route table.

14 dev pins + 5 server pins (red-first: E0432/E0425/E0599 over
`search_module_graph`/`with_module_graph`/`render_inspector_html_with_query`
and `render_html_search`), the `module_search`/`inspector_html_search`
criterion groups (≈89 µs at 1024 modules for the search — the
case-insensitive scan over module + importer paths; ≈36/140 µs for the
filtered render at 256/1024) and the `module_search` walkthrough example.

**What lands (M2, the F17 profiler + tier-map adapter).**
`crates/server/src/cost_endpoint.rs`: `ProfiledCostSource` — the layered
production `CostSource` the record's remaining halves ride: the build's
snapshot source decorated with the render profiler's per-source p50
(`renderMsP50`) and the pipeline's cache-tier map (`cache`), each keyed
by the same `?src=` spelling (`from_maps` takes the ledger's
`(source, p50)` / `(source, tier)` rows). Layering rules: the base
record is authoritative (a half the snapshot already carries is never
overwritten — the decorator decorates, it does not contradict the
build), a base miss stays the documented 404 (a path only the profiler
measured never fabricates a record), and empty maps leave the bytes-only
shape byte-identical. 8 TDD pins (red-first: E0432 over
`ProfiledCostSource`), the `cost_source_profiled` criterion group
(≈101 ns at 64 records — the base lookup plus the two map probes, the
decorator's overhead the snapshot group's delta) and the profiled section
in the `cost_endpoint` walkthrough example.

**Micro-debt (this diff, additive).**
`WaterfallRecorder::persist_to` (`crates/dev/src/lib.rs`) — the doc
promised "a write failure logs to stderr" while the code discarded every
failure (`let _ = std::fs::write`): the failure now warns through
`tracing` naming the path (the dev loop is a tracing pipeline — the
notify-fallback warn's own recorded rule), because a silently stale
side-car makes the MCP reader trust waterfalls the write never
delivered. The stale `RouteTableInspector` doc ("other inspector fields
(module graph …) stay empty") corrected — the module graph is wired now,
the `why`/cache/pool fields remain the honest-empty ones.

**Deltas recorded.**

- The inspector's module graph is BOOT-FROZEN: the serve boot harvests
  once (`build_hmr_graph`) and the per-tick rebuild stays the dev loop's
  (`cmd_dev`). The same posture the F02 islands scan took — the
  inspector answers from the boot graph until the wiring carries per-tick
  snapshots (the live/generation wire-up is recorded with the same
  posture).
- The search is case-insensitive ASCII (`to_ascii_lowercase`), like the
  classifier's `on`-prefix rule — a non-ASCII path casing differs only
  in a locale-specific fold no consumer needs.
- The `?q=` search is server-rendered (the form submits, the arm
  filters) rather than client-side JS over the state JSON: one matcher
  (the Rust function), no parity surface, no external assets on the
  inspector page.
- The `render_html_search` default delegates to `render_html` instead of
  being a required method: the trait's only in-tree implementor overrides
  it, and the default keeps a pre-search implementor (a third-party
  embed) honest rather than broken.

## The dev cost feed and the dev-plane attach land — the cost endpoint's records flow through a file the dev loop rewrites, and `oxide dev`/`oxide serve` finally answer `/__oxide/open` + `/__oxide/cost` (`feat/dev-cost-file-source` + `feat/dev-plane-cli-attach`, 2026-11-17)

**What lands.**

The §V2-F17 endpoint had a wire contract (`GET /__oxide/cost?src=`, the
`CostSource` seam), a build-side producer (`cost::source_costs`) and two
adapters (`SnapshotCostSource`, `ProfiledCostSource`) — but no dev-side
feed and no CLI attach: the endpoint answered nothing in the real dev
loop because the boot fed neither an `EditorOpener` nor a `CostSource`
(the recorded "remaining CLI half", plus the recorded "the cost feed
needs the dev-side snapshot design" gap). Two milestones close it.

**M1 — the records-file seam + the dev-side feed (`feat/dev-cost-file-source`).**
`crates/bundler/src/cost.rs`: `SourceCost` gains `Deserialize` and the
records file has one writer and one reader — `write_source_costs`
(pretty JSON, the `source_costs` order riding through, a trailing
newline, the parent dir created; two writes over the same records are
byte-equal, the build's determinism property riding the file) and
`read_source_costs` (`NotFound` on a missing file, `InvalidData` on an
unparsable one — the dev loop stays up either way). `crates/server/src/cost_endpoint.rs`:
`FileCostsSource` — the dev-side `CostSource` over the persisted file
(the `client.costs.json` spelling beside the client manifest). The dev
boot cannot hold the build's `BundleOutput` across rebuilds, so the boot
attaches once and a rebuild's rewrite is picked up by the NEXT query.
The staleness check is the cargo/make `(mtime, length)` stat PAIR — an
mtime-only check proved flaky where the timestamp tick is coarser than a
fast rewrite (two writes landing in one tick served the stale cache in
the red-first pin; the length half catches those, and a rewrite that
moves neither half carries identical content anyway). Honesty rules:
a hit maps the row one-to-one into `bytes_only` (a `null` chunk is the
honest empty string), a missing file is the no-build shape (misses, no
log noise), an unreadable file warns once per stat move naming the path,
a vanished file warns and clears (stale rows never outlive the file that
carried them). 5 bundler pins + 6 server pins (red-first: E0432 over
`write_source_costs`/`read_source_costs` and `FileCostsSource`), the
`cost_source_file` criterion group (the steady-state hit = one stat
pair + the cached lookup, at the 64/4096 record shapes), and the
file-backed section in the `cost_endpoint` walkthrough example.

**M2 — the dev-plane attach (`feat/dev-plane-cli-attach`).**
`crates/cli/src/build/bundle_client.rs`: the build phase persists the
records beside the client manifest after the gates pass (inside the
`!skip_client` guard — a fingerprint-match skip leaves the previous
file, the bundle is byte-identical so its records are). A write failure
warns; the build continues (the file is dev-plane telemetry, the
endpoint answers its documented 404s without it).
`crates/cli/src/dev/client_bundle.rs`: the dev bundle path writes the
same file after a FULL scan — a partial (`changed`) scan's output
carries the rebuilt modules, not the whole module map, and writing it
would clobber the full-build records with a partial table, so the last
full build's file stays until the next full scan.
`crates/cli/src/serve/start.rs`: the pipeline literal attaches both
endpoints beside the inspector under the same `with_inspector` gate —
`OpenEditorEndpoint::new(root, CommandOpener)` (the production opener;
the confinement and the coded `DEV-OPEN-001/002` refusals are the
endpoint module's) and `CostEndpoint::new(FileCostsSource::from_path(dist/client/client.costs.json))`.
Production never attaches either (the gate is the dev plane's), so the
default pipeline stays byte-identical.

**Deltas recorded.**

- The feed is a FILE, not an in-memory snapshot: the dev loop rebuilds
  in-process (`bundle_incremental`) and the serve boot has no handle on
  the bundler's incremental worker state — the persisted records file is
  the seam both sides already share (the same posture as the
  `client.manifest.json` contract), and the stat-pair check keeps the
  endpoint's answer current without a re-attach.
- Partial-scan rebuilds do not rewrite the records file: the
  `bundle_incremental` output for a `changed` scan is the rebuilt
  modules, not the whole map. The endpoint answers the last full build's
  records until the next full scan (an HMR edit followed by a
  save-everything passes a full scan and corrects the table).
- The dev-plane endpoints attach under `with_inspector` (the boot's
  dev-plane gate, the same flag the inspector and the dev token ride) —
  there is no separate config key; a serve without the inspector is
  production-shaped by definition.
- The inspector's live + generation rows (`with_live`/`with_generations`
  at the boot) remain the recorded CLI tail: the `LiveState` is built
  after the pipeline literal today and the boot-frozen tag snapshot
  would render an always-empty table — the honest attach needs the hub
  sampled per render (a `CliInspector` field) or the literal hoisted,
  which is a boot-order refactor this session kept out of the milestone.

## The §20 trace context gets consumed — the dev-plane trace feed and the `traceresponse` response half land (`feat/dev-plane-trace-feed` + `feat/trace-response-header`, 2026-11-17)

**What lands.**

§20 says "W3C trace context propagated into `fetch` op calls and user
code". The request half landed long ago — `trace_context.rs` parses the
`traceparent` header at the pipeline entry and rides
`OxideRequest.trace` — but the module's own doc recorded the honest
gap: the context was *available* and *not consumed by any live call
site* (the `fetch` op itself is the §4.5 stub). Two milestones consume
it.

**M1 — the dev-plane trace feed (`feat/dev-plane-trace-feed`, V2-F21
minted).** `crates/server/src/trace_feed.rs`: the `TraceSink` record
seam (`record` + `snapshot`), `RingTraceSink` (the bounded production
impl — capacity-capped FIFO ring, oldest evicted at capacity, snapshot
newest-first, a zero capacity records nothing) and `RequestTrace::of`
(one completed request's row: request id, method, path, status,
wall-clock duration, and the parsed trace halves — an untraced request
records the `None` halves honestly rather than being skipped).
`TracesEndpoint` answers `GET /__oxide/traces` with the rows as
camelCase JSON (the §14 convention); a non-GET verb is the documented
405 + `allow: GET` (the nav/open contract); there is no query
parameter — the window is the answer. The pipeline records on EVERY
completed request: the native arms (Raw/Static/LocalImage/RemoteImage/
ServerFn) and the dispatch arm all call `record_request_trace`, whose
`Option` check IS the production cost (no sink → no row, no
allocation, byte-identical). The §V2.J map gained its 13th row
(`/__oxide/traces`, dev plane, F21; the row-count pin moved 12 → 13)
and `docs/reference/endpoints.md` regenerated with it. The dev boot
attaches ONE `RingTraceSink` at `TRACE_FEED_CAPACITY` (256) to both
halves under the `with_inspector` gate — the WaterfallRecorder's
one-instance-two-readers posture; production never attaches. The feed's
snapshot is a point-in-time read taken BEFORE the query's own row
exists (the row lands on completion — the endpoint never fabricates a
record for a request whose status is not yet known), so a query answers
the pre-query state and its own row rides the NEXT query's answer (the
red-first pin encoded the wrong expectation and was corrected to the
honest contract). 17 server pins red-first (E0432/E0609/E0599 over
`RingTraceSink`/`RequestTrace`/`TRACE_FEED_ENDPOINT_PATH`/
`trace_sink`/`with_trace_*`), the `trace_sink_ring`/
`trace_feed_snapshot`/`trace_feed_serve` criterion groups, and the
`trace_feed` walkthrough example.

**M2 — the `traceresponse` response half (`feat/trace-response-header`).**
`crates/server/src/trace_context.rs`: `to_traceresponse` (the W3C
Trace Context Level 2 DRAFT's response header — `00-<trace-id>-<response-id>-<flags>`,
the same grammar as `traceparent`; the draft is NOT a Recommendation,
recorded here so nobody cites it as one) and `response_span_id` (the
response span id derives from the SEC-02 per-request id's first 16 hex
— deterministic per request, unpredictable across requests, no second
RNG draw; a malformed id degrades to no header, never a fabricated
span id). The dispatch arm emits it on a traced response: never over a
header the dispatch already set, never on an untraced request
(byte-identical), and scoped to the dispatch arm only — the
framework-rendered path where §20's "user code" answers (static/image/
server-fn arms stay header-free, recorded as the scoping decision).
The `trace_context.rs` module doc's "available but not consumed" note
is now the drift it warns about, and was rewritten to the two live
consumption paths (the `fetch` op remains the §4.5 stub). 8 pins
red-first (E0599 over `to_traceresponse`/`response_span_id`), the
`trace_response_format` criterion group, and the response-half section
in the `trace_feed` example.

**Deltas recorded.**

- V2-F21 is a MINTED work package: F01–F20 were all taken (F18 the
  load-bench scripts, F19 the capability-permissions spike, F20
  attested builds), and the trace feed is new spec work riding §20's
  observability row rather than a deferral of an existing package.
- The feed records EVERY completed request — including health probes
  and the dev-plane endpoint calls themselves. A bounded ring evicts
  honestly; a dev loop with traffic turns the 256-row window over in
  seconds and the probes cannot fill it alone. The alternative (skip
  native arms, skip probes) would need a taxonomy the spec does not
  define; the window is the honest answer.
- The record point lives in `serve_request`'s match arms rather than
  the `finish_response` helpers: the helpers lack the path, the trace
  context and the start instant, and threading three more parameters
  through two signatures for one call site each was the worse trade.
  The dispatch arm records AFTER `finish_response_full_async` so the
  row's duration covers the full serve path and the status is final.
- The `traceresponse` id derives from the request id instead of a
  fresh RNG draw: one response per request, the request id is already
  SEC-02 CSPRNG, and the deterministic derivation is testable. The
  emission is gated on the L2 draft's shape (16-hex response id), so a
  counter-fallback request id (the RNG-failure branch) degrades to no
  header.
- Micro-debt cleaned in the same diff: the `crates/cli/Cargo.toml`
  V2-A02 comment claimed `oxide-fw-dispatch` was "declared but unused"
  while `commands/replay.rs` impls its `ReplayExecutor` today; the
  `build_rewrite_map` doc still said "Phase 1b stub: files missing
  until rolldown lands" after rolldown and the prebundle landed. Docs
  drift fixed: `.github/PULL_REQUEST_TEMPLATE.md` and
  `CONTRIBUTING.md` named `cargo audit` while CI's gate is cargo-deny
  (licenses + advisories); `benchmarks/README.md` documented the
  criterion gate as `baseline-v01.json … 5` while the live gate is
  `after-wave-b.json … 10`.

## The trace rows leave the process — the tracing export seam and the feed's filtered window land (`feat/trace-export-tracing` + `feat/trace-window-query`, 2026-11-18)

**What lands.**

The F21 handoff recorded two tails: "the CLI's observability module
could feed a `TraceSink` bridge into a tracing layer" and the `fetch`
op's outbound `traceparent`. The second tail turned out to be already
implemented — `op_oxide_fetch` reads the worker thread's current
`traceparent` and adds it to the outgoing headers (a caller-set header
wins) — but *dormant*: no live call site invokes `set_trace_parent`
(the V8 op args do not carry the inbound headers; the Node backend
propagates through the loader payload). The module doc in
`trace_context.rs` still claimed the op was "still a stub", the
opposite drift, and now records the wired-but-latent truth. Two new
milestones close the first tail and widen the feed's read contract.

**M1 — the trace rows' tracing export (`feat/trace-export-tracing`,
V2-F22 minted).** `crates/server/src/trace_export.rs`:
`TracingTraceSink` is the export arm — one `tracing::info!` event per
recorded row under the module-path target
`oxide_fw_server::trace_export`, snake_case fields mirroring the row's
Rust field names (`request_id`, `method`, `path`, `status`,
`duration_ms`, `sampled`, `trace_id`, `parent_id`, `tracestate`), the
absent W3C halves rendered as the empty string (the row's `None`
halves stay recorded — the feed's own honesty rule, now on the event
stream as on the wire). Whatever subscriber the host installed renders
it: the console formatter, `OXIDE_LOG=json`, and with
`OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream — the rows correlate with
the §20 `oxide.request` span through the same `request_id` the
`Server-Timing` header reports. The sink is not a window:
`snapshot()` stays honestly empty (rows are emitted, never retained).
`FanoutTraceSink` is the record fan — the pipeline has ONE
`trace_sink` slot, so the dev boot that wants window + export needs a
sink that is both: `record` fans to every child, `snapshot`
concatenates each child's newest-first rows in construction order with
no cross-child re-ordering (`RequestTrace` carries no global sequence,
so any merge sort would be fabricated; the window child is listed
first, so a fanout-backed endpoint renders the ring's JSON exactly).
The CLI dev boot attaches fanout(ring, `TracingTraceSink`) only when
`OXIDE_TRACE_LOG` opts in (`crates/cli/src/serve/start.rs`, the
`trace_log_enabled` gate in `observability.rs` accepting anything but
`""`/`0`/`false`/`off`) — the endpoint stays ring-backed and the
default console output is byte-identical; production never attaches
(the F21 posture holds). 11 server pins red-first (E0432 over
`TracingTraceSink`/`FanoutTraceSink`), the `trace_export_record` and
`trace_export_fanout` criterion groups, and the `trace_export`
walkthrough example.

**M2 — the trace feed's filtered window (`feat/trace-window-query`,
V2-F23 minted).** F21's contract was "no query parameter — the window
is the answer"; a consumer that asks "which requests failed recently?"
or "what did the POSTs cost?" pulls 256 rows and filters by hand. The
contract widens: `TraceQuery` parses `?status=5xx` (a response-class
spelling) or `?status=404` (exact), `?method=` (case-insensitive
exact), `?path=` (a percent-decoded, segment-honest prefix — `/api`
matches `/api/x` but not `/apiology`), `?limit=` (positive, capped at
`TRACE_FEED_CAPACITY`, the nav endpoint's optional-limit shape). The
filters AND together, the newest-first order is never re-ordered,
unknown parameters are ignored by construction (the D41 `token` rides
the same query and the dev-plane arm consumes it before the endpoint
sees anything), and an invalid optional value degrades to no filter —
never an error. `TracesEndpoint::serve_query` renders the filtered
window; `serve` delegates with `None` and stays the whole-window F21
contract byte-identical; the dev-plane arm passes the raw query
through. The §V2.J map row gained the query spelling and the spec's
Appendix V2.J + the WP status table gained the F21/F22/F23 rows (the
F21 rows were minted last session but never landed in the spec doc —
drift fixed here). 18 server pins red-first (E0432 over `TraceQuery`,
E0599 over `serve_query`), the `trace_feed_query` criterion group, and
the filtered-window section in the `trace_feed` example.

**Deltas recorded.**

- The empty-string rendering for the absent W3C halves is a decision:
  a combinatorial event shape (emit the field only when present) needs
  three branches per row for the three `Option` halves, and a
  subscriber visiting fewer fields makes "was it absent or was the
  sink wrong?" unanswerable. The empty string keeps one event shape,
  mirrors the W3C grammar (an absent `tracestate` is the empty
  tracestate) and stays honest the way the row is.
- The fanout snapshot is honest concatenation, not a merge: rows carry
  no cross-child sequence number, so a globally sorted window would be
  fabricated order. The window child listed first keeps every existing
  endpoint contract intact (the tracing child contributes no rows).
- The `OXIDE_TRACE_LOG` gate keeps the export arm opt-in: a dev
  session's console would otherwise grow one line per request (the
  events render at info, inside the default `RUST_LOG=info` filter).
  Production never attaches either half — the unattached pipeline's
  record point is still one `None` check, byte-identical.
- Micro-debt cleaned in the same diff: the `observability.rs` module
  doc was the split_cli.py orphan (`run`'s buffered-stdout doc glued
  onto `init_tracing`, with a literal `).///` artifact) — moved home to
  `args.rs::run`, `init_tracing` and `env_filter` got their own docs.
  The badge counter (`scripts/count-tests.sh`) and the launch-content
  gate (`scripts/check-launch-content.sh`) both missed the 31
  flavoured `#[tokio::test(flavor = …)]` attributes — both count the
  `(\(|\])` shape now, and the README badge/prose re-measured (2730 →
  2792; the prose row said 2,155).
- Docs drift fixed: the README status rows (95/100 → ~93 per scorecard
  r9; 20/17 jobs → 24/21; `oxide gc`/`oxide bisect` dropped from the
  top-level Operate list — they are `oxide cache gc` /
  `oxide perf bisect`); `benchmarks/README.md` ("baseline-v01.json …
  is the file future Wave B work diffs against" → the live gate diffs
  against `after-wave-b.json`; the six-crate sweep vs CI's two-crate
  bench-gate shape made explicit). The committed docs gate was red at
  HEAD: `docs/reference/cli.md` predated the Fmt/Replay/Deploy commands
  and `docs_tdd.rs::page_registry_lists_the_reference_pages` still
  pinned the 4-page registry after `endpoints.md` joined — the pin
  moved to 5 and the page regenerated with the `docs generate --check`
  gate green.

## The dev-plane log feed and the trace rows' file export (F24 + F25, 2026-11-18)

**What landed (`feat/dev-plane-log-feed` + `feat/trace-file-export`).**

- **F24, the server half** (`crates/server/src/log_feed.rs`): the
  `LogSink` record seam + bounded `RingLogSink` (oldest evicted, snapshot
  newest-first, a zero capacity records nothing), the `CaptureLayer`
  capture arm — a `tracing_subscriber::Layer` that records every event
  the subscriber admits into the attached sink, `GET /__oxide/logs` as
  the §V2.J map's 14th row (dev plane, dev-token guarded, D41; 405 +
  `allow: GET` on a non-GET verb) and the `LogQuery` filter seam
  (`?level=warn,error` comma-separated exact names, `?target=` a target
  prefix, `?q=` a message substring, `?limit=` positive capped at the
  window; the filters AND together, unknown parameters are ignored, an
  invalid value degrades to no filter — the F23 posture). 21 TDD pins +
  3 module pins + the `log_feed` criterion group (ring record at
  capacity, the capture layer's per-event cost under a live subscriber,
  the snapshot, the serve render) + the walkthrough example.
- **F24, the CLI half** (`crates/cli/src/observability.rs`,
  `crates/cli/src/serve/start.rs`): `init_tracing` creates one
  `RingLogSink` and installs the capture layer over the subscriber it
  builds when the command is the dev boot (`set_dev_mode`'s gate — the
  same predicate the dev runtime rides); the serve boot attaches the
  `GET /__oxide/logs` endpoint over the SAME instance (the
  one-instance-two-readers posture the trace feed rides). Production
  never installs the layer: a non-dev command builds the same subscriber
  branches as before, byte-identical.

**Deltas recorded.**

- The capture layer needs no env gate, unlike `OXIDE_TRACE_LOG`: it is a
  second read on the event stream and changes no rendering, so the
  console output stays byte-identical even when attached — the gate is
  the dev-boot predicate itself, not an env var (a `serve` boot, which
  is the production serving path, never attaches).
- The feed never records its own module's events (the target
  `oxide_fw_server::log_feed` is skipped): the endpoint emits one
  serving event per poll, and a capture that recorded it would grow the
  window the poll renders.
- The wire renders the structured fields as an object with sorted keys
  (the default `serde_json::Map` order — the workspace never enables
  `preserve_order`, so the wire is deterministic bytes), and the
  `message` field is excluded from it (the message has its own slot).
- The capture wall clock is epoch milliseconds (`tsMs`), not ISO-8601:
  `chrono` is not a server-crate dependency, and the ring already reads
  newest-first — the consumer's timeline stays reconstructible across
  ring turnover without a second date formatting.
- **F25, the file arm** (`crates/server/src/trace_export.rs`,
  `crates/server/src/trace_feed.rs`): `FileTraceSink` appends one JSONL
  line per recorded row — the same camelCase row shape the endpoint
  serves (`row_json` promoted to `pub(crate)`, one row, one rendering,
  both surfaces), create + append at attach time, flush-per-record (the
  bytes are on disk before the record returns — a crash cannot lose the
  already-recorded rows). The attach is loud: a bad path aborts the dev
  boot rather than silently losing the audit trail (the session-keys
  posture). A mid-session write failure is sticky: one
  `tracing::warn!`, then the rows drop — a half-written line is worse
  than a stopped export. The snapshot stays honestly empty (the F22
  posture: an exporter, not a window). The CLI dev boot attaches it as
  the fanout's third child when `OXIDE_TRACE_FILE` opts in. 6 TDD pins +
  the `trace_export_file` criterion group + the file walk in the
  `trace_export` example.
- The sticky-failure rule is implemented and documented but not
  pinned: forcing a mid-stream write failure portably needs an
  injectable writer (a directory path fails at OPEN, before any record),
  and the trait's `record` returns `()` — the failure is observable only
  through the side effect. The honest alternative (threading a
  `Result` through `TraceSink::record`) would change every child sink's
  signature for a dev-only arm; the rule stays documented in the sink's
  docs.
- Micro-debt cleaned in the same diff: `image_jobs.rs`'s leader future
  replaced the `.expect("the image lanes are never closed")` panic with
  the coded `AST-JOBS` refusal (a closed lane is the honest diagnostic
  shape for the impossible case, not a panic in the future the pipeline
  awaits — the doc's # Panics section updated); the server crate's
  placeholder `mod integration {}` suite is a real cross-module pin now
  (one pipeline, two dev-plane windows, one token guard; every routed
  dev-plane path is in the map).
- Docs drift fixed: `docs/reference/endpoints.md` regenerated (the F24
  row; the table bytes match `endpoint_markdown_table()` verbatim), the
  spec's F-table gained the F24/F25 landed rows and V2.J the map row,
  the README badge/prose re-measured through `scripts/count-tests.sh`
  (2792 → 2824).

## The feed grows its own filter and the file arm's sticky rule gets pinned (F26 + F27, 2026-11-18)

**What landed (`feat/log-feed-filter-surface` + `feat/trace-file-writer-seam`).**

- **F26, the admission gate** (`crates/server/src/log_feed.rs`): the
  F24 polish tail — the capture arm's record-side filter. `LogGate`
  (the admission predicate — one pure call per admitted event),
  `AdmitAll` (the F24 default — every event the subscriber admits joins
  the window, byte-identical) and `LevelFloor` (events below the floor
  never join the window — the capture-side narrowing: the `LogQuery`
  seam narrows the RENDER, the gate narrows the RECORD),
  `CaptureLayer::with_gate` (the gate travels the layer's `Clone`; the
  no-feedback rule keeps precedence — the feed's own target stays
  skipped under any gate). The WIDENING half is the CLI's
  `OXIDE_LOG_FEED_LEVEL` composition
  (`crates/cli/src/observability.rs`): the host filter rides the
  formatter and export layers per-layer (`Layer::with_filter`, no
  global filter on the registry) and the capture layer admits its own
  floor — the console stays byte-identical while the window captures
  the events the console never renders (a dev loop with the console at
  `info` and the feed at `debug`). Unset (or falsy, or an unknown name
  — one stderr notice) keeps the F24 subscriber shape byte-identical.
  11 TDD pins (`log_feed_gate_tdd.rs`: the flagship pin proves the
  console/window split with the same primitives the boot builds) + the
  `log_feed_capture_gate` criterion group (the `AdmitAll` pass-through
  vs the `LevelFloor` comparison vs the dropped-event early return) +
  the gate section in the `log_feed` walkthrough example.
- **F27, the injectable writer** (`crates/server/src/trace_export.rs`):
  the F25 sticky-failure rule's recorded tail — the JSONL framing rides
  ONE constructor over any `std::io::Write`:
  `FileTraceSink::from_writer` (the seam the pins steer; `create` stays
  the file-path shape over the same framing, a `BufWriter` either way
  so the syscall shape matches). The sticky rule is now PINNED
  (`trace_file_writer_tdd.rs`, 8 pins): a steered writer fails
  mid-stream — the failed row never reaches the sink (no half line),
  exactly one warn event names the stopped export (pinned through the
  F24 capture arm itself — the observability surface rides its own
  pins), every subsequent record leaves the writer untouched, the
  snapshot stays honestly empty through the failure, and the injected
  writer's line is the endpoint's row shape verbatim (the wire identity
  rule). The file arm section in the `trace_export` example walks the
  shape end to end.

**Deltas recorded.**

- `tracing::Level`'s `Ord` is REVERSED from the intuitive severity
  order (`error` sorts smallest, `trace` largest — the ordering is
  "filter verbosity", not severity), so "at or above the floor" is
  `event level <= floor`. The first gate draft used `>=` and its own
  pins caught it red — the doc comment on `LevelFloor` records the
  footgun so the next reader does not re-derive it.
- The widening composition widens the feed only as far as its GATE
  allows: with no global filter on the registry the subscriber hands
  every event to every layer, so a gate-less composition would widen
  the window to everything (the pins hold `the_widening_composition_is_
  only_as_wide_as_its_gate`). The gate is load-bearing in the boot's
  composition, not scaffolding — the CLI passes the floor both
  in-layer and through the per-layer `filter_fn` (the outer filter is
  the cheap pre-dispatch skip; the in-layer gate is the invariant).
- The env gate's invalid spelling degrades loudly-but-honestly: one
  stderr notice naming the value, then the F24 shape (the falsy
  spellings stay silent — the `OXIDE_TRACE_LOG`/`OXIDE_TRACE_FILE`
  gate posture).
- The F25 record's "implemented-but-unpinned" debt is closed: the
  sticky rule's pins ride the injectable writer, not a directory-path
  open failure (which fails at OPEN, before any record — the honest
  alternative the F25 record weighed and deferred).
- Micro-debt cleaned in the same diff: the cache crate's placeholder
  `mod integration {}` suite is three real cross-module pins now (the
  canonical digest names the tier row and the freshness window decays
  as the policy promises; the single-flight owner/follower hand-off
  over the tiered store with the RAII guard's publish; the soft/hard
  tag invalidation across both tiers) — the assets placeholder suite
  remains the recorded debt.
- Docs: the spec's F-table gained the F26/F27 landed rows, the README
  badge re-measured through `scripts/count-tests.sh`, the log feed's
  module docs record the two-surface truth (the gate narrows the
  record, the per-layer composition widens the feed past the host
  filter). No new endpoint rows (no wire change — the gate is
  capture-side, the writer seam is sink-side), so the endpoints page
  needs no regeneration.

## The dormant fetch propagation wakes up and the feed grows its boot flag (F28 + F29, 2026-11-18)

**What landed (`feat/v8-traceparent-and-feed-flag`).**

- **F28, the §20 tail** (`crates/jshost/src/v8_ops.rs` + the wire-up):
  `set_trace_parent` was implemented but DORMANT — the recorded note said
  so verbatim ("Not yet called from a live site"; the trace id would have
  to be threaded through `CallInput` per op). The chain runs end to end
  now. The extraction — `trace_parent_from_call_input` — mirrors the Node
  worker's `traceparentFromPayload` for V8 parity: the host-threaded
  `CallInput.headers` map first (case-insensitive key), then the
  payload's host-validated `trace.traceparent` (the value the CLI derives
  per hop), then the payload's raw `headers` pairs; the W3C shape check
  is the Node worker's `TRACEPARENT_RE` VERBATIM (2-32-16-2 lowercase
  hex groups — parity, not breadth), and a malformed value is DROPPED,
  never forwarded (garbage must not reach upstream APM pipelines).
  `TraceParentGuard::for_call` is the per-call RAII the worker loop
  installs around every `V8Msg::Call` — seed on construction, CLEAR on
  drop (the Node worker's per-op stash rule; the worker runs one call at
  a time, so clearing — not restoring — is the honest hygiene, and a
  previous traced call's value never leaks into the next untraced one on
  the reused worker thread). The guard lives through the streaming drive
  in the worker-loop arm, so a late `fetch()` during a pending render or
  streaming loader propagates too. The request-driven hosts stamp the
  validated value into `CallInput.headers` at boot —
  `LoaderHost::run`/`run_streaming`, `EndpointHost::run` (the
  `LoaderRunArgs.trace` path), `FrameworkHost::render` + the streaming
  variant (the render envelope's `trace`/`headers`) — through
  `payload_trace_headers`, whose empty map means UNTRACED, nothing
  fabricated. The `fetch` op's append moved into the extracted
  `propagate_trace_parent` (one behavior, one function: the site-set
  `traceparent` wins, an absent slot leaves the outgoing headers
  untouched) so the op, the pins and the example hold the same rule.
  21 TDD pins (`trace_parent_tdd.rs`: the extraction spellings, the
  guard cycle, the append rule, and the wire-up captured through a fake
  `Pool` — `LoaderHost`/`EndpointHost`/`FrameworkHost` stamping verified
  on the `CallInput` that reaches the pool). The first jshost bench
  target joins (`benches/trace.rs` behind the crate's new `bench-util`
  feature, the workspace gate shape): the extraction scan over a padded
  header map, the guard cycle, and the append rule. The
  `trace_propagation` example walks the three seams without spawning a
  worker. The real end-to-end V8 fetch propagation rides CI's workspace
  job per the recorded V8-link convention.

- **F29, the feed's boot flag** (`crates/cli/src/args.rs` +
  `crates/cli/src/observability.rs`): the F26 polish tail's other half —
  `oxide dev --log-feed-level <LEVEL>` beside the `OXIDE_LOG_FEED_LEVEL`
  env var. The flag is a command-level fact decided PRE-RUNTIME
  (`apply_startup_env` writes the `FEED_LEVEL_OVERRIDE` atomic — the
  `set_dev_mode` shape; no mid-async env writes, no set_var race), and
  `feed_filter_level` prefers it over the env var. The falsy-spellings
  gate and the loud-unknown-name notice are shared by both spellings
  through `feed_floor_from`, whose notice NAMES THE SOURCE
  (`--log-feed-level=verbose` vs `OXIDE_LOG_FEED_LEVEL=verbose`) so a
  typo is findable. 7 CLI pins (the parse shape, the override
  precedence, the falsy/unknown degrades, the env path unchanged, the
  `apply_startup_env` threading) — the CLI behavioral pins ride CI's
  nextest workspace job per the recorded V8-link convention;
  `cargo check -p oxide-fw-cli --all-targets` is the local gate.

- **Micro-debt + drift (additive).** The serve substrate's one weak-error
  site is gone: `snapshot_generation_sources`/`copy_tree` returned bare
  `Result<_, String>` — the refusals are coded `SRV-*` diagnostics now
  (`SRV-GEN-IO` for the missing `src/` tree and every copy failure with
  the path in the message, `SRV-GEN-DEPTH` for the depth guard), the two
  `start.rs` call sites render the diagnostic (`msg = %e`), and the
  registry gained the two rows (`docs/errors.toml`). The pin
  (`generations_snapshot_refusal_names_the_srv_code`) holds the code so
  a revert to bare strings is loud. Docs drift: the README's Security
  row claimed `cargo audit` + a `.cargo/audit.toml` reachability file
  that does not exist (the CI gate is cargo-deny — two unfixable
  upstream advisories suppressed with reachability analyses IN
  `deny.toml`); the row names cargo-deny now. The `graph` crate's
  missing consumer example landed (`crates/graph/examples/pipeline.rs` —
  the demand → memoization → invalidation walk over an env source, the
  shape `build_graph.rs` drives in production).

## The cache plane's observability arms land (F30 + F31, 2026-11-18)

**Branch:** `feat/cache-observability-arms` (uncommitted changes; `ship.toml`
+ `.github/pr-body.md` gitignored). **Mint:** the WP table's F30/F31 rows —
both are the F28 cache observer seam's named follow-ons, so the seam's own
doc was the work plan.

- **F30, the metrics exporter** (`crates/cache/src/metrics.rs`): the seam
  doc's "attach a custom impl for the structured end — a metrics exporter"
  is now an in-crate type. `MetricsCacheObserver` aggregates every completed
  event into monotone counters (per-tier hits L0/L1/L2, stale hits, misses,
  sets, deletes, served/stored bytes, invalidations + affected keys);
  `CacheMetricsSnapshot` is the `Clone`+`Eq`+`Default` value mirror;
  `render_prometheus` renders the §20 text exposition in the request plane's
  `Metrics::render` shape. The family prefix is `oxide_cache_plane_*` — the
  request plane's `oxide_cache_l0_*` page-render rows are a DIFFERENT cache,
  and the module doc says so, so one scrape carries both without a label
  collision. Counters SATURATE at `u64::MAX` (`fetch_update` +
  `saturating_add`) — the pin caught the first draft's `fetch_add` wrap, and
  a saturated counter reads "at or past the bound" honestly. No new error
  codes: the seam is infallible (the F21 posture for a seam without
  refusals). 8 pins in `metrics_tdd` (incl. push/pull parity with `stats()`
  over real traffic), the `cache_metrics_exporter`/`cache_metrics_render`
  criterion groups, the `metrics` example.

- **F31, the bounded lossy feed** (`crates/cache/src/feed.rs`): the seam
  doc's "a heavy consumer should enqueue into its own channel and process
  elsewhere — the `FileTraceSink` posture is the model" made real.
  `EventFeed` enqueues owned FIFO `(key, op)` rows behind a bounded queue;
  the host drains with `try_pop`/`drain` from its own task. The DECISIONS:
  saturation drops the ARRIVING row (the stream posture — a stalled consumer
  keeps what it can still receive; a retention ring would silently drop the
  OLD rows), the FIRST drop leaves one sticky warn under `oxide::cache`
  (warn state under the same lock as the queue, so two producers
  saturating together warn exactly once), a zero capacity retains nothing
  (the `RingLogSink` posture, the warn still fires), and `dropped()` keeps
  the score. 9 pins in `feed_tdd` (incl. the warn-once capture through a
  steered `MakeWriter` and the two-producers-vs-draining-consumer honesty),
  the `cache_event_feed` criterion group, the `event_feed` example.

- **Docs drift fixed (additive).** The `observer` module doc's prescriptions
  ("a metrics exporter", "a heavy consumer should enqueue…") described
  consumers that did not exist in the crate — both now point at the in-crate
  types they name. `crates/cache/README.md` (the crate's rustdoc landing
  page, `include_str!`'d) never gained the F28 seam in its "What it
  provides" list — the observability bullet names all four shapes now
  (pull `stats()`, tracing, metrics, feed). The README badge re-measured
  2883 → 2904 by `scripts/count-tests.sh` (+21: the 17 TDD pins + the 4
  module tests).

- **Micro-debt cleaned (additive).** `crates/cache/src/http_handler.rs`'s
  `invalidate_tag` body serialization carried the last silent
  `unwrap_or_default()` in the file — the sibling arms were made loud in
  earlier sessions ("loud, not silent") but this one was missed; the
  impossible path warns with the code, tag and error now (behavior
  unchanged: the bool map cannot fail to serialize).

## The cache plane's posture pull and the coalesce parity row land (F32 + F33, 2026-11-18)

**Branch:** `feat/cache-plane-health-and-coalesce` (uncommitted changes;
`ship.toml` + `.github/pr-body.md` gitignored). **Mint:** the WP table's
F32/F33 rows — F32 completes the §20 pull side the F28/F30/F31 push arms
left unbalanced, F33 closes the one row class the push channel still could
not see.

- **F32, the posture pull** (`crates/cache/src/health.rs`): `stats()` counts
  events and the F30 exporter aggregates them, but neither answers an
  operator's boot probe — "what is this cache stack": which tiers are
  attached, what each resident tier holds, the tag-index pressure, whether
  the observability seam itself is wired. `TieredCache::health()` samples
  every tier's live state into one value-shaped `CacheHealth`
  (`Clone`+`Eq`+`Default`+serde). Nothing is fabricated: an unattached tier
  reads `false`/`0`/`None`. The remote tier is an opaque
  `Arc<dyn CacheHandler>` — its identity rides the new defaulted
  `CacheHandler::handler_kind` introspection method (`"tiered"`, `"http"`,
  behind their features `"redis"`/`"s3"`, `"custom"` for host drivers), the
  trait seam that lets the posture name the driver without widening the
  trait's protocol surface. `render_prometheus` renders the §20 exposition
  in the F30 shape with GAUGE TYPEs — a scrape reads current posture, not
  deltas since attach, and the metric kind is what separates the two
  `oxide_cache_plane_*` halves on one scrape. The remote driver's kind is
  intentionally NOT a label: the kind is a per-stack constant the snapshot
  carries, and the numeric exposition stays stable when a host swaps
  drivers underneath. No error codes: the seam is infallible (the F21
  posture). 13 pins in `health_tdd` (incl. the exact `stats()` parity on
  the memory plane, the introspection table with the redis/s3 drivers
  feature-gated the way their suites gate, the byte-equal deterministic
  render, the JSON round-trip with the entry-weight rule documented), the
  `cache_health_snapshot`/`cache_health_render` criterion groups, the
  `health` example.

- **F33, the coalesce parity row** (`observer.rs` + `tiers.rs` +
  `metrics.rs`): the pull channel's `coalesced` counter had no push twin,
  and `record_coalesced` had NO production caller — only an in-crate test
  fed it while its doc claimed "the render layer's single-flight usage"
  (a consumer that does not exist in the tree; the CLI's
  `l0_lookup.rs` coalesced path keeps its own map and never reports). The
  row lands honestly: `CacheOp::Coalesce { followers }` is one aggregate
  row per completed flight, the emitter is the now key-carrying
  `TieredCache::record_coalesced(key, followers)` — the host's flight
  owner calls it on completion with the count
  `SingleFlight::publish_and_cleanup` returned — and the doc now
  describes the actual contract instead of naming a phantom caller. Every
  in-crate consumer inherits the row: `kind()` names it `"coalesce"`,
  the tracing observer emits it (debug, the hot-path arm) with the
  `followers` field, the exporter counts it into
  `oxide_cache_plane_coalesce_events_total` /
  `oxide_cache_plane_coalesced_followers_total` (the same saturating
  counters), and the feed carries it as any other row. `stats().coalesced`
  keeps its meaning — the pins hold the two-channel agreement. 7 pins in
  `coalesce_tdd`, the `cache_coalesce_record` criterion group (the
  attached-vs-no-observer record delta), the `coalesce` example (the
  inline fanout impl demonstrating the seam's custom-impl contract).
  The recorded follow-on: the CLI render layer's flight loop calling
  `record_coalesced` at its own coalesced site (a one-line wire-in when
  the CLI's V8-link gate is cheap to run).

- **Docs drift fixed (additive).** `docs/architecture.md`'s `cache` row
  predated the entire observability plane — it listed the §11 machinery
  and nothing from F28/F30/F31; the row now names the push seam, its
  three in-crate consumers, and the posture pull. `crates/cache/README.md`
  gained the posture bullet beside the observability bullet. The README
  badge re-measured by `scripts/count-tests.sh`.

- **Micro-debt cleaned (additive).** `crates/cache/src/s3_handler.rs`'s
  `get` bound `meta_len` into the match tuple and immediately discarded it
  (`let _ = meta_len;`) — the size cap already refused the oversized
  object before buffering, so the binding was dead scaffolding; the arm
  yields the bytes now and the discard line is gone.

## The seam composes and the feed narrows (F34 + F35, 2026-11-18)

**Branch:** `feat/cache-observability-fanout-and-gate` (uncommitted changes;
`ship.toml` + `.github/pr-body.md` gitignored). **Mint:** the WP table's
F34/F35 rows — F34 ships the composition arm the F22 request plane already
named, F35 transposes the F26 filter surface to the cache feed; together
they finish the "one slot, many shapes" story the F28 seam doc prescribes.

- **F34, the observer fanout** (`crates/cache/src/fanout.rs`): the
  `with_observer` slot holds ONE observer, the crate ships three consumers,
  and the `coalesce` example carried the workaround — a four-line inline
  `FanoutObserver` whose own doc said it copied the server crate's
  `FanoutTraceSink` posture. `FanoutCacheObserver` is that posture shipped
  once: one event reaches every child in construction order (the
  `FanoutTraceSink` rule — no cross-child re-order is attempted, events
  carry no global sequence), the borrowed event is RE-SHARED, not rebuilt
  (a fan over N children is N virtual calls and zero copies — the seam's
  zero-alloc promise kept for the composition), an empty fan is the honest
  no-op (it attaches, `children() == 0`, delivers nowhere — the posture's
  `observer_attached` reads true because the slot IS wired), and fans nest
  (a fan over fans is just children again). `FanoutCacheObserver::shared`
  is the `EventFeed::shared`-shaped convenience — the `Arc<dyn
  CacheObserver>` ready for `with_observer`. No error codes: the fan is
  infallible (the F21 posture). 8 pins in `fanout_tdd` (incl. the
  payload-re-shared pin over a stale serve and the real-traffic composite
  where the exporter's counters and the feed's FIFO describe the same
  events), the `cache_observer_fanout` criterion group (one/two/three-child
  fan delta + the composed hot path), the `fanout` walkthrough example, and
  the `coalesce` example switches to the shipped type.

- **F35, the feed's admission gate** (`crates/cache/src/feed.rs`): the
  queue retained EVERY row — right for a pipeline consumer, wrong for a
  narrow watcher: a deploy-watch consumer (`invalidate` + `coalesce` rows
  only) paid the enqueue for the whole hot-hit stream it would have
  drained and discarded, and a hot stream saturated its bound before the
  deploy rows it actually wanted arrived. The gate is the record-side
  narrowing (the server crate's F26 shape — `LogGate` narrowed the
  capture, this narrows the enqueue): `FeedGate` sees the borrowed event
  BEFORE the owned row is built, so a rejected row allocates nothing,
  never joins the queue, never counts in `dropped()` and never fires the
  sticky warn — admission is the feed's configured shape, not a
  degradation. `AdmitAll` is the default spelled as a type (row-for-row
  the ungated feed), `KindGate::only(&[kinds])` is the domain gate over
  `event.kind()`'s stable names, `EventFeed::with_gate` is the builder arm
  and `EventFeed::gated` the shared-shape sugar. The bound applies behind
  the gate unchanged: admitted rows saturate exactly as before (the
  warn-once rule holds — the gate and the bound compose). 9 pins in
  `feed_gate_tdd`, the `cache_event_feed_gate` criterion group (the admit
  delta over the ungated push + the reject floor), and the `event_feed`
  example gains the gated section.

- **Micro-debt cleaned (additive).**
  `crates/cache/src/single_flight.rs`'s `FlightOwnerGuard::publish` sent
  the entry to the followers and DROPPED the count the send produced
  (`let _ = tx.send(entry);`) — the count is exactly what the F33 emitter
  contract routes to `TieredCache::record_coalesced` ("the host's flight
  owner calls it on completion with the count the publish reached"), so
  the guard's owner had no honest join number to record; `publish` returns
  the `publish_and_cleanup`-shaped count now (`#[must_use]` with the
  emitter routing in the message) and two in-module pins hold the
  one-follower and zero-follower arms. The CLI render stage's owner site
  names the count it still drops (`let _dropped_join_count = …`) until the
  recorded wire-in lands. The same pass made every file this session
  touches pedantic-clean under clippy 1.100 (the session's added files and
  the touched bench sites; `examples/metrics.rs`/`examples/health.rs` and
  `tiers.rs`/`cache_tdd.rs` keep the crate's recorded residuals — the
  hygiene pass that opts the crate into `[lints] workspace = true` remains
  the recorded debt).

- **Docs drift fixed (additive).** `docs/architecture.md`'s `cache` row
  names the composition arm and the gate now; `crates/cache/README.md`'s
  observability bullet names the four consumer shapes and the gate. The
  README badge re-measured by `scripts/count-tests.sh`.

## The coalesce wire lands in production and the cache plane joins the §20 scrape (F36 + F37, 2026-11-18)

**What the spec asks.** §11.3's single-flight coalescing is observable (F33's
`CacheOp::Coalesce` rows) and §20's metrics endpoint exposes the cache plane.
Both seams were crate-complete and production-dead: `TieredCache::record_coalesced`
had no production caller (the F33 row's emitter doc named a consumer that did not
exist — the render stage's owner site named the count it dropped), and the §20
endpoint served only the request plane's `oxide_cache_l0_*` rows — the F30
exporter attached nowhere, so a real scrape carried no `oxide_cache_plane_*`
family at all.

**What lands.** F36 — the render stage's flight owner records the publish count
(`crates/cli/src/dispatch/stages/render.rs`): the typed `TieredCache` half of the
boot's shared tier rides `FrameworkStack` (`cache_tier_typed`,
`crates/cli/src/framework_shim.rs`) and `DispatchDeps` (`l0_tiered`,
`crates/cli/src/dispatch/deps.rs`, `Some` exactly when `l0` is — a recording half
for a plane with no flights is never fabricated) down to the owner site, where the
count `FlightOwnerGuard::publish` returns (the F33 follow-on's shape) lands in
`TieredCache::record_coalesced`. The response shapes are byte-identical — the wire
records, it does not reroute: the follower still rides `L0-coalesced`, a
zero-follower flight still records (one row, zero joins), and the un-wired boot
stays silent. F37 — the serve boot attaches the F30 exporter over the boot's tier
(`FrameworkStack::cache_exporter`, attached in `build_framework_stack` and
mirrored by the test-stack arm in `start_server`) and hands its scrape to the §20
endpoint through `Metrics::set_cache_plane`
(`crates/server/src/metrics.rs`): `render()` appends the closure's rows beside the
request-plane rows (appended last, one closure run per scrape — a pull reads the
posture NOW, never a value frozen at attach), and the unattached render is
byte-identical.

**Why not a typed dependency.** The server crate's dead `oxide-fw-cache`
dependency was removed deliberately (the recorded micro-debt); a `Metrics`-side
`Fn() -> String` slot keeps it out — the CLI (which already depends on both) does
the composition, and the server crate gains a generic extra-exposition seam the
next plane can ride. The closure indirection also keeps the slot honest at the
type level: the endpoint cannot reach into the cache plane, it can only append
what the boot handed it.

**Deltas recorded.** The exporter attach is unconditional at serve boot — the
hot-path delta is the F28-benched one-virtual-call cost, and the §20 endpoint then
exposes the families without a new config knob (the scrape is the knob). Dev-mode
boots attach nothing (no L0 plane, no flights). The coalesce row joins via the
flight's L0 store key (the P-04-reused `flight_key`, the canonical hashed
spelling) — the pipeline pins pin kind and count, not the hash. The CLI pins ride
the recorded V8-link convention (compile-verified locally, CI's nextest workspace
job runs them).

**Micro-debt cleaned (additive).** `crates/assets/tests/integration.rs` was the
recorded `mod placeholder {}` stub; it now pins three cross-module flows
(prerendered HTML → the JSON route index, byte-stable across builds; the SVG
sprite's referenced ids surviving the identity build shape with the preservation
assertion green; a scanned collection's broken-reference diagnostics naming the
missing target). The serve boot's `l0-seed.json` read — a PRESENT-but-corrupt
manifest was silently skipped (`if let Ok(bytes) … if let Ok(v)`) — now warns
loudly under `CACHE-SEED` naming the path; a MISSING manifest stays silent (a
build with no prerendered pages is normal).

**Docs drift fixed (additive).** The spec's WP table gained the F36/F37 landed
rows; `docs/architecture.md`'s cache/server/cli rows name the wire;
`crates/cache/README.md`'s observability bullet names the production consumers.
The README badge re-measured by `scripts/count-tests.sh`. The `boot_posture`
example walks the whole wire in one run (attach, fly, record, both expositions).

## The ledger, its gate and its wire — the record the last three sessions owed (F38 + F39 + F40 catch-up, 2026-11-19)

**What this section is.** The F38, F39 and F40 sessions appended their records
to `HANDOFF.md` and the spec's WP table but never to this file — the protocol
every earlier session kept. This is the catch-up, recorded from those sessions'
own handoff records (the authoritative source, committed beside this file), so
the deviation log's chain is whole again. Nothing here is new work.

**What landed (from the handoffs).** F38 — the F28 seam's dashboard-batcher
shape ships in-crate: `HotKeyObserver` (`crates/cache/src/hot_keys.rs`), the
bounded per-key event ledger with first-sight admission, the saturating
counts, the deterministic `top(n)` read, the sticky-refusal warn under
`oxide::cache` and the labeled `oxide_cache_plane_hot_key_total` exposition.
F39 — the F35 filter-surface shape transposed to the ledger:
`HotKeyGate`/`AdmitAllKeys`/`HotKeyKindGate::only` +
`with_gate`/`gated` — rejections are admission (never counted, never warned),
consulted on the borrowed event before the ledger lock. F40 — the ledger's
production wire: the CLI boot attaches it beside the F30 exporter behind the
F34 fan (`FrameworkStack::cache_hot_keys`, `CACHE_HOT_KEY_CAPACITY` = 2048)
and the §20 cache-plane scrape composes the exporter's families with the
ledger's labeled family through `cache_plane_scrape`; the server crate stays
cache-agnostic.

**Drift the catch-up repairs.** `docs/architecture.md`'s `cache` and `cli`
rows named the gate but not the wire — both now name F40 (and F41, below).
`crates/cache/README.md`'s hot-key bullet named F38/F39 but not the F40
production consumer — it names the wire now.

## The §20 hot-key top window is operator-tunable (F41, 2026-11-19)

**What the spec asks.** Nothing new — this is the F40 handoff's recorded
pickup verbatim: "thread an operator-tunable window through
`CACHE_PLANE_HOT_KEY_TOP` so the §20 scrape's top-n is config-driven."

**What lands.** The F40 scrape's top-n stops being a compile-time constant.
`crates/cli/src/observability.rs` gains `cache_hot_key_top()` (the
`OXIDE_CACHE_PLANE_HOT_KEY_TOP` gate, beside the `OXIDE_TRACE_FILE` /
`OXIDE_TRACE_LOG` / `OXIDE_LOG_FEED_LEVEL` gates it mirrors) over the pure
parse core `cache_hot_key_top_from(Option<&str>)`: the falsy spellings
(`""`/`0`/`false`/`off`) keep the F40 default (`CACHE_PLANE_HOT_KEY_TOP`,
unchanged), a parsed value rides verbatim clamped to the ledger's own bound
(`CACHE_HOT_KEY_CAPACITY` — rows past the occupancy do not exist, so the cap
keeps one scrape bounded), and garbage degrades loudly-but-honestly — one
stderr notice naming the source, then the default (the `feed_floor_from`
posture). `cache_plane_scrape` takes the window as a parameter and the boot
resolves it ONCE at the `set_cache_plane` wire
(`crates/cli/src/serve/start.rs` — the `trace_file_path` posture: a
boot-time read, not a per-scrape env poll). Window `0` renders the bare
family pair; the default shape is byte-identical to the const-rendered F40
scrape.

**Why a parameter, not ledger state.** The window is a BOOT decision about
the SCRAPE (the §20 endpoint's render), not a property of the attribution —
the ledger's contract (`top(n)` takes the caller's n) stays unchanged, the
F30 exporter's families are parameter-free, and a future consumer (a flag, a
config key) re-resolves at its own wire without touching the cache crate.

**Pinned.** `crates/cli/src/tests.rs` (V2-F41 section): the resolver's parse
core (unset/falsy/garbage/clamp — pure, parallel-safe), the one env-read pin
over the real spelling (the `feed_filter_level_still_reads_the_env_var…`
shape), the boot-observable composition and the window's two-sided bound
(0 → the bare pair, 1 → one labeled row). TDD red-first: the pins failed
with E0425 over the resolver fns and E0061 over the old 2-arg scrape before
the impl existed, then went green. Benches: the `cache_hot_key_top_window`
criterion group (`crates/cache/benches/hot_keys.rs`) — the read and render
delta across default/operator/cap scales over a full-capacity ledger. The
`hot_keys` example gains the operator-window section.
