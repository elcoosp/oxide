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
