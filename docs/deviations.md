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

**Status.** Substrate landed (`crates/cache/src/lib.rs::build_id_of_dist`,
`crates/dispatch/src/build.rs` — `GenerationSet`, `stage_build_check`,
`advertise_build`, `is_data_plane_path` — plus the additive
`render::hydration_bootstrap_with_build` / `Injector::with_build_id` and the
`packages/core/src/client/build.ts` client helpers wired into the router,
`enhance` and `__oxide_call_fn`). This follows the A02/A05/A06/F04 pattern:
the V8-free surface ships complete and pinned, the V8-linked wiring is
deferred.

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

1. **`phase_emit` does not compute or persist the id yet** — the
   `build_id_of_dist` call at the end of emit and the writes to
   `dist/build-id` / `.oxide/build-id` live in `cmd_build` (V8-linked).
   Until then `build_id_of_dist` is reachable from tests/benches only.
2. **No stage in the pipeline** — the closure does not call
   `stage_build_check` and responses are not passed through
   `advertise_build`; the data plane behaves byte-identically to pre-A04
   (the acceptance "absent header allowed" holds trivially).
3. **No generation promotion** — `cmd_dev` reload does not construct a
   `GenerationSet`, so the window semantics are pinned but not driven.
4. **The conformance assertion "build id advertised"** (HTML carries
   `build:`, `/__data` answers carry `x-oxide-build`) belongs to the wiring
   PR, together with the `packages/core` client bundle rebuild.

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

## V2-F05 live loaders — deltas and deferrals (substrate step)

**Status.** Substrate landed (`crates/server/src/live.rs`,
`crates/router/src/lib.rs::RouteLive` + scan,
`crates/config/src/lib.rs::LiveConfig`,
`crates/dispatch/src/build.rs` live data-plane contract,
`crates/render/src/lib.rs` bootstrap,
`crates/adapters/src/lib.rs::Capability::Live`,
`packages/core/src/client/live.ts`). Endpoint, rerun closure and the
client-router lifecycle are the A01-phase-3 wiring (V8-linked); see the
handoff for the one-sentence pickup list.

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

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

1. **No `GET /__oxide/live` endpoint in the pipeline yet** — the
   same-origin check, the route lookup, subscribe-or-503 and the
   `live_stream` body compose into the dispatch closure's stage list;
   the substrate functions are reachable from tests/benches only.
2. **No rerun closure** — the wiring calls the same data-route path as
   `/__data` (spec step 5) with the stored header snapshot so middleware,
   locals, layout loads and gating behave identically; `Runner` runs with
   an injected closure in tests. The `event: stale` emission on a failed
   rerun (redirect/error → `None` snapshot) is the wiring's watch on the
   cleared channel.
3. **No tag-revalidation hook** — the `TieredCache::invalidate_tag` call
   sites (server + action handlers' `revalidate.tag`) do not call
   `hub.affected(tag)` yet; the substrate pins the fan-out.
4. **No client-router lifecycle** — `connectLive` is exported from
   `client/live.ts` but the router does not call it after hydration yet
   (an EventSource hitting a not-yet-existing endpoint would 404-retry
   loop per page — worse than inert); the bootstrap already advertises
   `live:true` for opted-in routes and the field is read by the wiring.
5. **The `AST-LIV-001` refusal** (`live: true` on Static) belongs to the
   refusal-rule WP (spec §2776 `check_config`); the matrix row it consults
   is in place. The `tests/live.sh` integration script needs the running
   endpoint — deferred with it.
6. **Capability row choices the spec sentence leaves open**: Deno/Bun =
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

**What is deliberately deferred (A01 phase-3 / V8-capable host):**

- Nothing F06-specific: the transport is active end-to-end inside the
  substrate (server policy ⇄ SSE frames ⇄ client decode). The deferred
  wiring list from F05 (endpoint, rerun closure, revalidation hook, client
  router lifecycle, `AST-LIV-001` refusal, `tests/live.sh`) is unchanged.
- The `transport: 'poll'` fallback still polls full snapshots; converting
  poll responses to patches is pointless while ETags gate the round trip.

**Hygiene rides-along:** the `odf` crate-module doc pointed at a
nonexistent `docs/format.md` and a nonexistent `packages/oxide` package —
corrected to the real paths (`packages/core/src/client/odf-decode.ts`, the
fixtures directories), with a note recording the drift. The other drift
found this session (README crate count, CI job counts, spec Appendix V2.O's
all-`todo` table, CONTRIBUTING's toolchain claim) is listed in the handoff
and left to the doc owners.

**To unclose:** the A01 phase-3 wiring (the rerun closure already calls the
data-route path; its `Option<Bytes>` result now flows through
`prepare_rerun` automatically) — then the `tests/live.sh` e2e, which should
assert `event: data` frames carry `ODP1` after the first full snapshot.

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
