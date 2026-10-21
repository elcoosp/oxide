<!-- Part II of the Oxide specification (v0.2, DRAFT).
     This is the extension surface: D21-D42, wave plans, appendices.
     Part I (the frozen v0.1 audit target) is `spec-v0.1.md`.
     This file is a draft; the version banner at the top of each file
     is the authoritative statement of which is which. -->

# Part II. Oxide v0.2 Specification (implementation edition)

*Baseline: repository HEAD `3975f2a` (18 Oct 2026), 17 crates, 6 TypeScript packages, about 78k lines. Inputs: the full code dump, the three research reports (perf, compiler, deps), `docs/oxide-v0.2-feature-designs.md`, and the HANDOFF files. This part is written so an implementing agent with no prior context can execute each work package (WP) from its text alone. Where Part II conflicts with Part I, Part II wins for v0.2. Part I section numbers are written as §n; Part II sections are V2.n.*

## V2.0 Agent contract (read before touching code)

1. **Read order:** `CONVENTIONS.md`, the newest `HANDOFF*.md`, then the WP. If a statement here contradicts the tree, **trust the tree** and record the drift in `WORKLOG.md`.
2. **Locate by symbol, not by line number.** Line numbers below are as of `3975f2a` and will move. Each WP gives a `grep` command that finds the code.
3. **TDD is mandatory.** Write the failing test, run it and see it fail, then implement. Put tests where they do not pay the V8 link cost (CONVENTIONS §8): pure logic belongs in `oxide-router`, `oxide-cache`, `oxide-odf`, and the new `oxide-dispatch` crate (V2-A02). Do not add tests to the `oxide-cli` test target unless they need the in-process CLI.
4. **Own your files.** Each WP lists files to create and modify. Public API changes are additive. A change outside the list needs a one-line note in `WORKLOG.md`.
5. **Errors are diagnostics:** return `OxideError` with a stable code `PREFIX-NNN` and `.with_fix()` where a fix is known. New codes go in the Appendix V2.K registry.
6. **Commits:** one commit per file, conventional style, WP id in the body, no `feat(...)` on formatting-only diffs.
7. **Verify the emitted artifact, not the log.** Every WP has an *Observable proof*: a command whose output proves the behavior on a real build.
8. **Gates** before the final commit of a WP: `cargo fmt --all --check`; `cargo test -p <crate>` per touched crate; then `cargo test --workspace --no-fail-fast`, `pnpm test:packages`, `cd conformance && node run.ts`, `tests/fixtures/check.sh` and `tests/fixtures/check-v8.sh`. The redb CAS can collide across concurrent test binaries (`Database already open`); a clean re-run is authoritative.
9. **Environment rules:** no `timeout` on macOS (use a background PID plus a `sleep N; kill` watchdog); never `pkill -f oxide-cli`; kill servers you started by PID; canonicalize every `--root` once at command entry.
10. **Never:** run `git checkout -- examples/` when the change targets examples; double-append marker blocks (dedupe with `content.find(marker, first + len(marker))`); add a dependency that is not in the root `[workspace.dependencies]` table (add it there first, pinned).
11. **Performance WPs** require a before and after number from `criterion` or `benchmarks/`, pasted into `WORKLOG.md`. A perf change without a number is rejected.
12. **Feature WPs** also ship: a conformance assertion when user-visible, a capability-matrix row when target-dependent, a config key plus JSON Schema entry when configurable, and an `oxide doctor` check when it needs external tooling.

**WP template** used throughout: *ID, size, depends-on, goal, evidence, files, design (types and signatures), steps, code, tests (named), observable proof, acceptance, rollback, risks.* Sizes: **S** is at most one agent-day, **M** is up to a week, **L** requires a timeboxed spike with written kill criteria before the build.

## V2.1 Verified baseline at `3975f2a`

### V2.1.1 What v0.1 already ships (do not redo)

All seven Bucket-2 user-visible gaps are closed and proven end to end: router boot (`bootWithRouter` hydrates before attaching `createRouter`), nested layouts, `not-found.tsx` with HTTP 404, `error.tsx`, `route.ts` dispatch, `middleware.ts` with `locals` and `{rewrite}`, awaited `defer()` values, real hydration for Vue, Svelte and Solid, loading boundaries, real streaming SSR for React, Vue and Solid (Svelte falls back to `renderToString`), and manifest chunk preloading. Wave 1 (seven P0) and Wave 2 (fourteen P1) are closed: HMR `ox:data`, prerender layout chain and `parent_data`, a byte-identical TS ODF encoder, middleware headers on L0 hits, real `build_id`, HTTP `Alt-Svc`, `oxide check` with `tsgo` and `oxlint`, canonical `cache.l0.maxSize`, out-of-order deferred boundaries. Test baseline: Rust 0 failed targets; JS 299 pass; conformance 24 assertions x 4 adapters green; four `cargo-fuzz` targets scaffolded.

### V2.1.2 v0.1 work still open (carried into v0.2 as Wave A)

| Item | Evidence in tree | WP |
| --- | --- | --- |
| Parallel layout loads (§9.2) | `needs_parent` exists on `RouteEntry`, but the dispatch comment near `cli/src/lib.rs:4850` says loads are still sequential | A06 |
| Prerender error-status sidecar | Documented deviation in `write_prerender` (error page baked as 200) | A07 |
| Middleware chain outer to inner (§8.5) | Only the nearest ancestor is resolved (scan at about `lib.rs:4282`) | A05 |
| Redis and S3 cache handlers | `cache.handler` match arm near `lib.rs:6584` is a loud no-op for `redis` | A10 |
| V8 backend parity for `endpoint:run`, `middleware:run`, deferred awaiting, `renderStream` | Implemented in the Node worker only (HANDOFF bucket2 §What remains 3) | A08 |
| Plugin transforms on the prerender path | SPEC-17 known remainder | A09 |
| typegen `searchParams` from `searchSchema` | SPEC-09 remainder | E02 |
| Playwright for the four smoke fixtures | not automated | E10 |
| Stale artifacts | `benchmarks/report.md` (feeds the CI gate), `docs/architecture.md`, duplicate `WORKLOG.md` entries, stale comments in `jshost/src/v8_ops.rs` and the middleware `{rewrite}` comment, CI `push.branches` line check | A03 |
| README "wiring in progress" paragraph | Older than the handoffs; several items it lists are done. Re-verify each and rewrite | A03 |

### V2.1.3 The three research reports reconciled against the tree

| Claim in report | Status | Evidence and action |
| --- | --- | --- |
| perf #1: four O(routes) scans per request in `build_dispatch` | **Valid** | Middleware, not-found, error and layout-module scans inside the request closure (`build_dispatch`, `cli/src/lib.rs` from line 4042 to 5439; scans at about 4282, 4345, 4381, 4445). Each builds `format!("{base}/")`. WP B01 |
| perf #2: endpoint inputs built for every page GET | **Valid** | `endpoint_params_json`, `endpoint_headers`, `endpoint_body`, `endpoint_method_for_dispatch`, `endpoint_url_for_dispatch` (about 4332 to 4336). WP B03 |
| perf #3: `pattern_of` allocates per request | **Valid** | about 4239. WP B03 |
| perf #4: `nonce_for` called up to 5 times | **Valid** | `cli/src/lib.rs` about 4754, 4797, 5138, 5147, 5267. WP B03 |
| perf #5: `compress_body` blocks the reactor | **Stale, already fixed** | `spawn_blocking` at `server/src/lib.rs` about 3157. No action |
| perf: mimalloc not installed | **Stale, already fixed** | `#[global_allocator]` in `cli/src/main.rs:7`. No action |
| perf #6: `CacheKey::canonical` allocates a hex String | **Valid** | `cache/src/lib.rs:113-125`. WP B05 |
| perf #7: tag index `Vec` with `iter().any` and JSON round trip on disk | **Valid** | `cache/src/tiers.rs` about 246 (disk) and 531 (memory). WPs B05, B06 |
| perf #8: `precompile_entries` sequential | **Valid** | `cli/src/precompile.rs:230` `while let Some(path) = queue.pop_front()`. WP B07 |
| perf #9: `html.replace(&nonce, SENTINEL)` copies whole document | **Valid** | `cli/src/lib.rs` about 3449, 3527, 3611 (prerender) and 5306 (live fill). WP B08 |
| perf #10: layout ids recomputed in prerender | **Valid** | about 3294 to 3330. WP B01 |
| perf Tier 2: router scan is O(routes) and allocates | **Valid** | `router/src/matcher.rs:60-91` (`match_in`); `archive.rs:149` (`arch_match_impl`) also recomputes `layout_chain` per match. WP B02 |
| perf: `Metrics::observe` takes a write lock on first sight of a route | **Valid** | `server/src/metrics.rs:100`. WP B03 |
| compiler: Solid, Vue, Svelte compile through the Node tooling pool | **Valid** | `cli/src/precompile.rs` (`compile_solid`, `compile_vue`, `compile_svelte`); `compiler/src/framework.rs` `requires_tooling_pool`. WPs C01 to C04 |
| compiler: `solidjs-compiler` pins oxc 0.144 while workspace is 0.153 | **Unverified by this spec** | Crate APIs and pins are taken from the report. Every C-series WP begins with a spike that proves the API before any integration |
| deps: add `oxvg`, `fontcull`, `srcmap`, `tantivy`, `rimage`-style job model | **Plausible, unverified** | Same rule: spike, then adopt. WPs C06 to C09, F09 |

### V2.1.4 Structural findings that shape the plan

1. `crates/cli/src/lib.rs` is **7,697 lines**; `build_dispatch` alone is about **1,400 lines** and `cmd_build` about **1,100**. Every perf and feature WP touches them. Splitting them is the first WP (A01), and a V8-free `oxide-dispatch` crate (A02) makes their tests compile in seconds instead of minutes.
2. The **capability matrix** (`adapters/src/lib.rs`, `capability_matrix`) builds rows with `row([(Capability, FeatureSupport); 7])`. A fixed-size array literal means every new capability (rust functions, search, skew, delta, permissions) breaks every row. WP R02 changes it to a slice before adding rows.
3. **No SSE exists** in the tree (`grep text/event-stream` is empty), and no `x-oxide-build` header exists. Both are new.
4. Endpoint namespace is already split: `/__oxide/*` is the data plane and `/_oxide/*` is the ops plane (`/_oxide/{vitals,static,og,metrics,image,hmr,health,fn}`, `/__oxide/state`). New endpoints follow that rule.

## V2.2 Goals, non-goals and release gate

### V2.2.1 Goals

- **G1 Close v0.1.** Finish the open items in V2.1.2.
- **G2 Remove per-request O(routes) work** and measurable allocation from the hot path; publish numbers.
- **G3 Native compilers** for Solid, Vue and Svelte (and the React Compiler) behind a trait with a Node fallback.
- **G4 Ship the 20 v0.2 features** from the feature-designs document, with the two L-rated ones as spikes.
- **G5 DX and ecosystem:** HMR completeness, typed search params, real migration codemods, plugin-compat harness, first-party modules, SRI and Trusted Types, an agent-friendly surface (`llms.txt`, MCP tools).
- **G6 Keep the conformance promise:** every user-visible feature lands with a conformance assertion on all four adapters.

### V2.2.2 Non-goals for v0.2

React Server Components (D17), partial prerendering implementation (format reserved only), parallel and intercepting routes (tree format already reserves slots), a Rust UI adapter, replacing Playwright or the Go-based `tsgo`.

### V2.2.3 Measurable targets (validate, then publish in `benchmarks/report.md`)

| Metric | Target |
| --- | --- |
| Route match at 1,000 routes, p99 | at least 10x faster than `match_in` baseline; zero heap allocations on the static-route path |
| Per-request O(routes) scans in dispatch | 0 (asserted by a test that counts iterations) |
| L0 hit, heap allocations per request | at most 12 (measured with `dhat`), down from the baseline recorded in B09 |
| Cache tag insert with 10k keys under a tag | O(1) amortized |
| Cold build, 500 modules, Solid or Svelte or Vue | at least 3x faster with native compilers than with the Node pool (same machine, cold CAS) |
| Warm build | unchanged (22 to 41 ms measured) |
| Live loader fan-out, one revalidation to second tab | under 200 ms |
| Skew window | old tab data calls succeed for `deploy.skewWindow` after a deploy |

### V2.2.4 v0.2 release gate

1. Waves A and B complete; numbers published.
2. Every F-series feature merged, or explicitly spiked with a written keep or kill decision.
3. Conformance: the existing 24 assertions plus one new assertion per user-visible feature, green on four adapters on Linux, macOS and Windows.
4. Capability matrix has rows for every new capability and the refusal rule covers them.
5. `cargo test --workspace --no-fail-fast`, `pnpm test:packages`, conformance, the V8 fixture check and the benchmark gate are all green.
6. README, `docs/architecture.md` and the spec reflect reality (no claim contradicted by the tree).
7. Security review of the new endpoints (live, stream, rust fn, search, nav) completed.

### V2.2.5 Contract versioning

The adapter contract stays `contract: 1`. v0.2 adds optional fields only (`capabilities.mixedIslands`, `framework` on island hosts, `hydrateIsland` must be real). A new optional `contractMinor?: number` field starts at 1; the core refuses an adapter whose `contractMinor` is higher than the core knows.

## V2.3 Decision record (D21 to D42)

| ID | Decision | Rationale |
| --- | --- | --- |
| D21 | Split `crates/cli/src/lib.rs` into modules, then extract a V8-free `oxide-dispatch` crate for pure request logic | CONVENTIONS §8: V8 link makes `oxide-cli` tests about five minutes per iteration |
| D22 | Precompute a `RouteContextTable` (layout chain, middleware chain, error, not-found, loading, module paths) once at boot from the route tree | Dispatch results are pure functions of the route id |
| D23 | Router speedup is a **candidate index plus the existing exact scorer**, not a rewritten matcher | Preserves exact scoring and tie-break semantics by construction; the old matcher becomes the property-test oracle |
| D24 | Cache keys are 128-bit `KeyHash` values (truncated blake3); hex only at display and filename boundaries | Removes two allocations per lookup |
| D25 | Tag index is `HashSet` per tag plus a reverse key-to-tags map; the disk tier uses a redb multimap table instead of a JSON list | O(1) insert, no read-modify-write of JSON |
| D26 | Native framework compilers live behind `NativeCompiler`; config `framework.compiler` is `auto`, `native` or `node`; `auto` picks native when compiled in and healthy | Fast path without a hard dependency; Node stays a fallback for one release |
| D27 | Pre-1.0 and git-only compiler crates are **vendored** under `vendor/` as path dependencies, pinned by commit | Hermetic CI; RC-track upstreams cannot change output silently |
| D28 | Build id is the canonical digest of the dist manifest; every data-plane request carries `x-oxide-build`; two server generations are retained | Keystone for skew, delta deploys, prefetch graph and offline cache |
| D29 | Server push for live data uses SSE; WebSocket stays for HMR and user sockets | One-way data, no handshake, proxy friendly |
| D30 | ODF gains a single new container tag `PATCH` (next free tag in `crates/odf/src/wire.rs`) | The only new wire format; everything else reuses ODF |
| D31 | Resumable streams are an in-memory `StreamStore` with byte offsets, LRU and TTL; expiry returns 410 | Correctness over cleverness |
| D32 | Brownout state is one `AtomicU8` read per request | No lock on the hot path |
| D33 | Native Rust server functions are loaded as a `cdylib` through `libloading` (model A); frozen deploys use a fat-binary build (model B) | Dev loop speed vs deploy simplicity |
| D34 | Search is a feature-gated crate (`oxide-search`, `tantivy`); static export keeps a JSON index | Do not tax compile times for non-users |
| D35 | Capability permissions are **enforced on V8** and **advisory on Node**, and the matrix says so | Honest degradation |
| D36 | Attestation: ed25519 via `ring`, CycloneDX SBOM from lockfiles, in-toto provenance | Reuses existing hashes |
| D37 | Interaction replay is core-owned, about 1 KB, framework-agnostic | Works for all four adapters at once |
| D38 | Subresource Integrity for client chunks and an opt-in Trusted Types policy | Content hashes already exist |
| D39 | `Capability` rows become slice-based; adding a capability never edits existing rows | See V2.1.4 item 2 |
| D40 | Every JS plugin transform and every native compiler result is CAS-keyed with implementation id and version | Switching `framework.compiler` never reuses a stale artifact |
| D41 | New dev-only surfaces are dev-token guarded (SEC-09 pattern) | Existing precedent |
| D42 | Spikes (F02 auto-islands, F19 capabilities, every C-series API check) have written kill criteria and a decision record in `docs/decisions/` | Prevents silent scope creep |

## V2.4 Work package index

IDs: **A** foundations, **B** performance, **C** compilers and toolchain, **F** the 20 v0.2 features (numbering from `docs/oxide-v0.2-feature-designs.md`), **E** DX and ecosystem, **R** release. B09 lands first (it records the baseline) even though its number is last.

| ID | Work package | Size | Depends on | Main files |
| --- | --- | --- | --- | --- |
| A01 | Split `cli/src/lib.rs` into modules | M | none | `crates/cli/src/*` |
| A02 | Extract V8-free `oxide-dispatch` crate | M | A01 | new `crates/dispatch` |
| A03 | Hygiene bundle (CI, bench report, docs, worklog, comments) | S | none | `.github`, `benchmarks`, `docs`, `README.md` |
| A04 | Build id and generations substrate | M | A01 | `cli`, `bundler`, `server` |
| A05 | Full middleware chain and unified locals threading | M | A02 | `cli`, `jshost`, worker |
| A06 | Parallel layout loads (§9.2) | S | A02, B01 | `dispatch`, `cli` |
| A07 | Prerender error-status sidecar | S | A01 | `cli`, `server` |
| A08 | V8 backend parity for endpoint, middleware, deferred, renderStream | M | none | `jshost` |
| A09 | Plugin transforms on the prerender path | S | A01 | `cli`, `plugin` |
| A10 | Redis and S3 cache handlers (feature-gated) | M | none | `cache`, `cli` |
| B01 | `RouteContextTable` | S | A02 | `router`, `dispatch` |
| B02 | Candidate index and allocation-free matcher | M | none | `router` |
| B03 | Hot-path hoists (endpoint inputs, nonce, label, metrics) | S | A01 | `dispatch`, `server` |
| B04 | Shared request parts (`Arc`) | S | A01 | `server`, `cli` |
| B05 | `KeyHash` and HashSet tag index | S | none | `cache` |
| B06 | Disk-tier tag multimap | S | B05 | `cache` |
| B07 | Parallel precompile | S | none | `cli/src/precompile.rs` |
| B08 | Nonce splice without full copy | S | A01 | `cli`, `render` |
| B09 | Benchmark baseline, criterion harness, allocation counting | M | none | `benchmarks`, each crate `benches/` |
| C01 | `NativeCompiler` trait, config, selection, CAS keys | M | none | `compiler`, `config`, `cli` |
| C02 | Solid native compiler | M | C01 | `compiler`, `vendor/` |
| C03 | Vue native compiler | M | C01 | `compiler` |
| C04 | Svelte native compiler | M | C01 | `compiler` |
| C05 | React Compiler (optional) | S | C01 | `compiler` |
| C06 | Source-map compose in the plugin chain | S | none | `bundler`, `plugin` |
| C07 | SVG optimizer | S | none | `assets` |
| C08 | Glyph-usage font subsetting | M | none | `assets` |
| C09 | Parallel image job model | S | none | `assets` |
| C10 | MDX native evaluation (spike) | M | none | `assets` |
| C11 | `oxfmt`, `oxlint --type-aware`, `tsrs` opt-in | S | none | `cli` |
| F01 | Mixed-framework islands | M | A04 | `packages/*`, `cli`, `render` |
| F02 | Auto-islands inference (spike) | L | F01 | `compiler` |
| F03 | Interaction replay | S | none | `packages/oxide` |
| F04 | Cache-safe personalization | M | A05 | `server`, `cli`, `cache` |
| F05 | Live loaders (SSE) | M | A04, A05 | `server`, `cli`, `packages/oxide` |
| F06 | ODF PATCH deltas | M | F05 | `odf`, `packages/oxide` |
| F07 | Resumable streams | M | none | `server`, `cli` |
| F08 | Native Rust server functions | M | R02 | new `crates/macros`, `cli` |
| F09 | Built-in search (tantivy) | M | R02 | new `crates/search` |
| F10 | Skew protection and atomic deploys | M | A04 | `cli`, `jshost`, `adapters` |
| F11 | Delta deploys | M | A04, R02 | `adapters` |
| F12 | Brownout mode | M | B03 | `server` |
| F13 | Learned prefetch | M | A04 | `server`, `packages/oxide` |
| F14 | Offline-first module | M | A04 | `packages/oxide`, `cli` |
| F15 | Record and replay | M | A05 | `cli`, `jshost` |
| F16 | Why-dynamic with quick fixes | S | none | `compiler`, `cli` |
| F17 | Component cost overlay | M | E01 | `dev`, `packages/oxide` |
| F18 | Performance bisect | S | B09 | `cli` |
| F19 | Per-route capability permissions (spike) | L | A08 | `jshost` |
| F20 | Attested reproducible builds | S | A04 | `cli` |
| E01 | Complete HMR client and state-preserving HMR | M | none | `dev`, `packages/oxide` |
| E02 | Typed search params, typed `href` and `goto` | M | none | `router`, `packages/*` |
| E03 | Real `oxide migrate` codemods | M | none | `cli` |
| E04 | SRI, Trusted Types, CSP report-only | S | none | `render`, `server` |
| E05 | Plugin-compat harness and published table | M | A09 | `plugin`, `conformance` |
| E06 | Module API v1 and first-party modules | M | none | `plugin`, `packages/*` |
| E07 | `llms.txt` and expanded MCP tools | S | none | `cli` |
| E08 | Dev experience v2 (overlay, inspector, doctor) | M | E01 | `dev`, `cli` |
| E09 | Edge completion (core-wasm ops, Cloudflare e2e) | M | A08 | `core-wasm`, `adapters` |
| E10 | Playwright automation for the four fixtures | M | none | `tests/`, `conformance` |
| E11 | Docs site generation from schema and spec | M | none | `packages/`, `docs` |
| E12 | Monorepo and workspace support | S | none | `config`, `dev` |
| E13 | Svelte streaming SSR (spike) | S | none | `packages/svelte` |
| R01 | Conformance v2 (new assertions) | M | each F | `conformance` |
| R02 | Capability matrix v2 (slice rows, new capabilities) | S | none | `adapters` |
| R03 | Refresh benchmark report and gate | S | B-series | `benchmarks` |
| R04 | Docs, README, architecture refresh | S | all | `docs`, `README.md` |
| R05 | Security review checklist | M | F05, F07, F08, F09, F13 | `docs/security` |
| R06 | Release checklist and changelog | S | all | repo root |

### V2.4.1 Dependency graph and parallel tracks

```
Track 1 (structure):   A01 -> A02 -> B01 -> A06
                              \-> A05 -> F04, F15
                       A01 -> B03, B04, B08, A07, A09 -> E05
Track 2 (algorithms):  B09 (first) -> B02 | B05 -> B06 | B07 | R03
Track 3 (compilers):   C01 -> C02 | C03 | C04 | C05      C06 C07 C08 C09 C10 C11 (independent)
Track 4 (keystone):    A04 -> F10 -> F11 | F13 | F14 | F20 | F05 -> F06
Track 5 (features):    F03 | F07 | F16 | F18 | E02 | E04 | E07 (independent starts)
Track 6 (platform):    R02 -> F08, F09, F11
Track 7 (client):      E01 -> F17, E08          A08 -> F19, E09
Gate:                  every F -> R01 ; everything -> R04, R05, R06
```

### V2.4.2 Recommended execution order

1. **Week 0:** B09 (baseline numbers), A03 (hygiene: CI line, stale report), R02 (matrix refactor, 2 hours).
2. **Weeks 1 to 2:** A01, A02, then B01, B03, B04, B05, B08 in parallel by different agents (they touch different files after A01).
3. **Weeks 2 to 4:** B02, B06, B07, A04, A05, A06, A07, A09, F03, F16, E07.
4. **Weeks 3 to 6:** C01 then C02 to C05 in parallel; A08, A10, E01, E02, E04.
5. **Weeks 5 to 10:** F01, F04, F05, F07, F10, F12, F20, F18, E03, E05, E06.
6. **Weeks 9 to 14:** F06, F08, F09, F11, F13, F14, F15, F17, E08, E09, E10, E11, spikes F02, F19, C10, E13.
7. **Weeks 13 to 16:** R01, R03, R04, R05, R06, release.

Conflict rule for parallel agents: after A01 each module file has one owner per WP; if two WPs need the same file, the earlier-numbered WP in this table merges first and the later one rebases.

## V2.5 Wave A: foundations

### V2-A01 Split `crates/cli/src/lib.rs` (M, no dependencies)

**Goal.** Turn a 7,696-line file into a module tree with zero behavior change, so every later WP edits small files.

**Evidence.** `fn cmd_build` is 1,053 lines (`2731-3783`), `fn build_dispatch` is 1,396 lines (`4042-5437`), `start_server_inner_full` 432, `cmd_dev` 393, `write_prerender` 246. 85 top-level items hold 7,051 of the 7,696 lines.

**Files.** Create the modules listed in the MAP inside `scripts/split_cli.py` below; modify `crates/cli/src/lib.rs`; add `scripts/split_items.py`, `scripts/split_cli.py`, `scripts/dist-digest.sh`.

**Step 0. Record the baseline digest** (before touching anything):

```bash
# scripts/dist-digest.sh <app-dir> : deterministic digest of dist/ (builds are byte-stable per project)
set -euo pipefail
cd "$1/dist"
find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1
```

Run `cargo build -p oxide-cli`, then for each `examples/smoke-app-*`: `oxide-cli build` and `scripts/dist-digest.sh <app>`. Save the four digests in `WORKLOG.md`.

**Step 1. Phase 1, mechanical split.** Add the two scripts, run the plan, then apply:

```python
# scripts/split_items.py - lossless top-level item index for a rustfmt-formatted Rust file.
# An item starts at a column-0 line (fn/struct/enum/impl/mod/use/...) and ends at the next
# column-0 line that is exactly a closing brace; one-line items end on the same line.
# Doc comments and attributes directly above an item belong to it.
import re, sys
NL = chr(10)
START = re.compile(r'^(pub([(][a-z]+[)])?[ ]+)?(async[ ]+)?(unsafe[ ]+)?(fn|struct|enum|trait|impl|mod|const|static|type|use|macro_rules!)(?![A-Za-z0-9_])')
NAME = re.compile(r'(?:fn|struct|enum|trait|mod|const|static|type|macro_rules!)[ ]+([A-Za-z_][A-Za-z0-9_]*)')

def index(src):
    lines = src.split(NL)
    items, i, n, run = [], 0, len(lines), None
    while i < n:
        l = lines[i]
        if l.strip() == '':
            run = None; i += 1; continue
        if l.startswith('//') or l.startswith('#['):
            if run is None: run = i
            i += 1; continue
        m = START.match(l)
        if not m:
            run = None; i += 1; continue
        start = run if run is not None else i
        run = None
        kind = m.group(5)
        nm = NAME.search(l)
        name = nm.group(1) if nm else (l.strip()[:60] if kind == 'impl' else '')
        one_line = l.rstrip().endswith(';') or (l.rstrip().endswith('}') and l.count('{') == l.count('}'))
        if one_line:
            items.append((start, i, kind, name)); i += 1; continue
        j = i
        while j < n:
            if j > i and lines[j].rstrip() in ('}', '};', ');', ')'):
                break
            j += 1
        items.append((start, j, kind, name)); i = j + 1
    return items

if __name__ == '__main__':
    text = open(sys.argv[1]).read()
    its = index(text)
    print('# items=%d covered=%d total=%d' % (len(its), sum(e - s + 1 for s, e, _, _ in its), len(text.splitlines())), file=sys.stderr)
    for s, e, k, nm in its:
        print('%d-%d%s%d%s%s%s%s' % (s + 1, e + 1, chr(9), e - s + 1, chr(9), k, chr(9), nm))
```

```python
# scripts/split_cli.py - V2-A01 phase 1. Usage: python3 scripts/split_cli.py crates/cli/src [--apply]
# Moved items become pub(crate); every new module starts with `use crate::*;`, and lib.rs re-exports
# each module with `pub(crate) use m::*;` so existing names keep resolving (tests.rs included).
import re, sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from split_items import index
NL = chr(10)
MAP = {
 'args': ['SecretsOp', 'Command', 'CacheOp'],
 'observability': ['init_tracing', 'env_filter', 'is_secret_key', 'log_request', 'build_otel_layer'],
 'config_load': ['load_config_or_default', 'load_config_strict', 'has_explicit_render_config',
                 'refine_render_modes_from_ast', 'scan_routes', 'param_name', 'pattern_of',
                 'walk_files', 'read_json', 'config_rules_to_router'],
 'boot_entry': ['generate_island_boot_entry', 'jsx_import_source_for', 'inject_island_boot_script'],
 'bundle_env': ['bundle_env_options'],
 'dev/hmr': ['hmr_route_data_update'],
 'dev/client_bundle': ['ensure_dev_client_bundle'],
 'dev/cmd': ['cmd_dev', 'start_dev_server_full', 'start_dev_server'],
 'emit/edge_wasm': ['compile_edge_wasm'],
 'emit/search': ['emit_content_search_indexes'],
 'emit/sitemap': ['emit_sitemap_and_robots'],
 'emit/fonts': ['emit_fonts'],
 'emit/snapshot': ['persist_diagnostics', 'emit_snapshot_lint', 'emit_snapshot_blob'],
 'emit/config_bundle': ['emit_config_bundle'],
 'emit/pwa': ['emit_pwa_worker', 'emit_web_manifest'],
 'prerender': ['url_path_from_rel', 'write_prerender'],
 'data_route': ['route_data_response'],
 'build/secret': ['build_secret'],
 'build/cmd': ['cmd_build'],
 'serve/stream': ['start_server', 'render_stream_to_body_stream', 'render_stream_with_recovery',
                  'probe_render_error_status', 'chunk_signals_error', 'parse_form_urlencoded', 'action_from_query'],
 'dispatch/entry': ['build_dispatch'],
 'endpoints/image_og': ['build_og_endpoint', 'build_image_endpoint'],
 'endpoints/dispatchers': ['build_server_fn_dispatcher', 'build_action_dispatcher', 'cfg_socket_routes',
                           'build_endpoint_dispatcher', 'build_middleware_dispatcher', 'build_socket_dispatcher'],
 'hooks': ['build_css_transform_hook', 'build_js_plugin_hook', 'build_transform_hook', 'load_client_assets'],
 'serve/start': ['start_server_inner', 'start_server_inner_full', 'resolve_bind_host', 'resolve_port', 'cmd_serve'],
 'commands/inspect': ['cmd_routes', 'cmd_analyze', 'cmd_check', 'which', 'cmd_why', 'cmd_doctor'],
 'commands/ops': ['cmd_secrets', 'cmd_cache', 'cmd_audit', 'cmd_test', 'cmd_mcp'],
 'commands/author': ['cmd_add', 'cmd_generate', 'cmd_migrate', 'cmd_schema', 'cmd_config', 'find_config_file'],
 'commands/render': ['cmd_render'],
}
VIS = re.compile(r'^(pub([(][a-z]+[)])?[ ]+)?(async[ ]+)?(unsafe[ ]+)?(fn|struct|enum)(?![A-Za-z0-9_])')

def main():
    src_dir = sys.argv[1]
    apply = '--apply' in sys.argv
    path = os.path.join(src_dir, 'lib.rs')
    text = open(path).read()
    lines = text.split(NL)
    by_name = collections.defaultdict(list)
    for s, e, k, nm in index(text):
        if k in ('fn', 'struct', 'enum') and nm:
            by_name[nm].append((s, e))
    want = [n for ns in MAP.values() for n in ns]
    missing = [n for n in want if n not in by_name]
    dup = [n for n in want if len(by_name.get(n, [])) > 1]
    print('missing:', missing, 'duplicates:', dup)
    assert not missing and not dup, 'MAP drifted from the tree: fix the names'
    print('items to move:', len(want), 'lines:', sum(by_name[n][0][1] - by_name[n][0][0] + 1 for n in want), 'of', len(lines))
    if not apply:
        return
    drop = set()
    for n in want:
        s, e = by_name[n][0]
        drop.update(range(s, e + 1))
    for mod, names in MAP.items():
        chunks = []
        for n in names:
            s, e = by_name[n][0]
            body = lines[s:e + 1]
            for i, l in enumerate(body):
                if VIS.match(l):
                    if not l.startswith('pub'):
                        body[i] = 'pub(crate) ' + l
                    break
            chunks.append(NL.join(body))
        fp = os.path.join(src_dir, mod + '.rs')
        os.makedirs(os.path.dirname(fp), exist_ok=True)
        open(fp, 'w').write('//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).' + NL + NL + 'use crate::*;' + NL + NL + (NL + NL).join(chunks) + NL)
    keep = [l for i, l in enumerate(lines) if i not in drop]
    tops = sorted({m.split('/')[0] for m in MAP})
    decl = ['// V2-A01: module tree; moved items are re-exported so existing names keep resolving.']
    for t in tops:
        decl += ['mod %s;' % t, 'pub(crate) use %s::*;' % t]
        kids = sorted({m.split('/')[1] for m in MAP if m.startswith(t + '/')})
        if kids:
            open(os.path.join(src_dir, t, 'mod.rs'), 'w').write('//! V2-A01 module root.' + NL + NL.join('mod %s;%spub(crate) use %s::*;' % (k, NL, k) for k in kids) + NL)
    open(path, 'w').write(NL.join(keep).rstrip(NL) + NL + NL + NL.join(decl) + NL)
    print('applied; now run: cargo check -p oxide-cli')

main()
```

Commands: `cp scripts/split_items.py crates/cli/src/` is NOT needed (the script imports from its own directory). Run `python3 scripts/split_cli.py crates/cli/src` (plan, must print `missing: [] duplicates: []`), then add `--apply`, then `cargo fmt -p oxide-cli && cargo check -p oxide-cli`.

**Expected compile fixes (all mechanical):** (a) an item that referenced a sibling private helper now needs `pub(crate)` on that helper (add it); (b) two moved modules both named `cmd` are fine because they are different paths, but never glob-re-export two items with the same name (rename the second); (c) `impl` blocks (`ImageEndpointBackend`, `ServerFnDispatcher`, `ActionDispatcherImpl`, `EndpointDispatcher`, `MiddlewareDispatcher`) stay in `lib.rs` in phase 1; move each next to its `build_*` function in `endpoints/dispatchers.rs` by hand.

**Step 2. Phase 2, split `cmd_build` into phases.** In `build/cmd.rs`, introduce:

```rust
pub(crate) struct BuildCtx {
    pub root: std::path::PathBuf,          // canonicalized once (CONVENTIONS: path canonicalization)
    pub cfg: oxide_config::Config,
    pub tree: oxide_router::RouteTree,
    pub build_id: String,
    pub out: std::path::PathBuf,           // dist/
    pub prerendered: std::collections::BTreeMap<String, Vec<String>>,
    pub seed_bodies: Vec<(String, String, String)>,   // (route_id, url_path, sentineled html)
    pub w: Box<dyn std::io::Write + Send>,
}

pub(crate) async fn cmd_build(root: &str, /* existing args */) -> anyhow::Result<()> {
    let mut cx = build::phase_load(root, /* args */)?;      // canonicalize, config, scan, typegen
    build::phase_bundle_client(&mut cx).await?;
    build::phase_bundle_server(&mut cx).await?;
    build::phase_prerender(&mut cx).await?;
    build::phase_seed_l0(&mut cx)?;
    build::phase_emit(&mut cx).await?;                      // snapshot, sitemap, fonts, pwa, deploy artifacts
    Ok(())
}
```

Find phase boundaries by the existing comment banners (`grep -n "^    // " crates/cli/src/build/cmd.rs`). Each phase becomes a function in its own file under `build/`; shared locals become `BuildCtx` fields. Move one phase per commit; after each, re-run Step 0 digests (they must still match).

**Step 3. Phase 3, split `build_dispatch`.** Replace the 18-parameter function with a deps struct and stage functions in `dispatch/`:

```rust
// dispatch/deps.rs
pub(crate) struct DispatchDeps {
    pub tree: std::sync::Arc<oxide_router::RouteTree>,
    pub archived: Option<std::sync::Arc<oxide_router::ArchivedTree>>,
    pub metrics: std::sync::Arc<oxide_server::Metrics>,
    pub proxy_rules: Vec<oxide_dev::ProxyRule>,
    pub page_renderer: Option<std::sync::Arc<dyn oxide_server::PageRenderer>>,
    pub loader_runner: Option<std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    pub routes_root: std::path::PathBuf,
    pub boot_script_url: Option<String>,
    pub l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    pub actions: Option<std::sync::Arc<dyn oxide_server::ActionDispatcher>>,
    pub pool: Option<std::sync::Arc<dyn oxide_jshost::Pool>>,
    pub i18n_cfg: Option<oxide_assets::i18n::I18nConfig>,
    pub endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_server::EndpointDispatcher>>,
    pub middleware_dispatcher: Option<std::sync::Arc<dyn oxide_server::MiddlewareDispatcher>>,
    pub dev_hmr: bool,
    pub vary_headers: Vec<String>,
    pub waterfall: Option<oxide_dev::WaterfallRecorder>,
    pub session_codec: Option<std::sync::Arc<oxide_server::SessionCodec>>,
}

// dispatch/ctx.rs : the per-request mutable state that today lives as 60+ locals in the closure
pub(crate) struct RequestCtx {
    pub started: std::time::Instant,
    pub req: oxide_server::OxideRequest,
    pub path: String,                    // normalized
    pub locale: String,
    pub data_route: Option<String>,      // Some(route) when GET /__data
    pub matched: Option<oxide_router::MatchResult>,
    pub locals: serde_json::Value,       // middleware locals (A05 makes this a typed struct)
    pub extra_headers: Vec<(String, String)>,
    pub timings: StageTimings,
}
```

Stage functions, in call order, each `async fn stage_x(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow` where `enum Flow { Continue, Respond(oxide_server::OxideResponse) }`: `stage_prelude` (rewrite-hop guard, pool gauges, proxy, path normalize, locale, data-route detection), `stage_match`, `stage_middleware`, `stage_data_route`, `stage_action`, `stage_l0_lookup`, `stage_loaders`, `stage_endpoint`, `stage_render`, `stage_finish`. The closure becomes: build `RequestCtx`, then `for stage in STAGES { if let Flow::Respond(r) = stage(d, &mut cx).await { return r } }`. Move **one stage per commit**; after each commit run the digest check, `tests/fixtures/check.sh` and the smoke-app e2e (`--ignored`).

**Tests.** Phase 1 and 2: the existing suite is the test. Add `scripts/split_selfcheck.sh` that fails if any moved item text differs from `git show HEAD:crates/cli/src/lib.rs` after stripping the leading ` pub(crate)  `. Phase 3: a unit test per stage using fakes (this is what A02 enables).

**Observable proof.** The four `dist-digest` values equal the Step 0 values; `wc -l crates/cli/src/lib.rs` is under 800; no file under `crates/cli/src` exceeds 700 lines except `dispatch/stages/*.rs` at the end of phase 3, which must each be under 300.

**Rollback.** Each phase is a separate PR. Phase 1 is a pure move and can be reverted wholesale.

### V2-A02 Extract the V8-free `oxide-dispatch` crate (M, depends on A01)

**Goal.** Put request-handling logic that does not need V8 in a crate whose tests link in seconds (CONVENTIONS §8).

**Why it works.** `oxide-server` already owns the traits (`LoaderRunner`, `PageRenderer`, `ActionDispatcher`, `EndpointDispatcher`, `MiddlewareDispatcher`) and depends only on `oxide-cache`, `oxide-router`, `oxide-render`, `oxide-config`, `oxide-odf`, not on `oxide-jshost`. A crate that depends on those can orchestrate stages against fakes.

**Files.** Create `crates/dispatch/{Cargo.toml,src/lib.rs,src/url.rs,src/flow.rs,src/chain.rs,src/loads.rs}`; add `crates/dispatch` to `[workspace] members` and `oxide-dispatch = { path = "crates/dispatch", version = "0.1.0" }` to `[workspace.dependencies]`; add `oxide-dispatch` to `crates/cli/Cargo.toml`.

```toml
# crates/dispatch/Cargo.toml
[package]
name = "oxide-dispatch"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
description = "V8-free request orchestration: URL helpers, stage flow, middleware chain, load graph (V2-A02)"

[dependencies]
oxide-diagnostics = { workspace = true }
oxide-router = { workspace = true }
oxide-server = { workspace = true }
serde_json = { workspace = true }
futures = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }
bytes = { workspace = true }
```

**Initial contents (move, do not rewrite).** `url.rs` receives `parse_form_urlencoded` and `action_from_query` verbatim from `serve/stream.rs`, plus these small pure helpers (new, with tests):

```rust
// crates/dispatch/src/url.rs
pub const MAX_REWRITE_HOPS: u32 = 8;

/// Path without query or fragment.
pub fn path_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or("/")
}

/// Trailing slash removed except for the root.
pub fn normalize_path(raw: &str) -> String {
    if raw.len() > 1 && raw.ends_with('/') { raw.trim_end_matches('/').to_string() } else { raw.to_string() }
}

/// `Some(route)` when the request is `GET /__data?route=...` (default route is `/`).
pub fn data_route_of(method: &str, path: &str, url: &str) -> Option<String> {
    if !method.eq_ignore_ascii_case("GET") || path != "/__data" { return None; }
    let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let r = q.split('&')
        .find_map(|kv| kv.strip_prefix("route=").map(oxide_server::percent_decode))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/".to_string());
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn normalize_keeps_root() { assert_eq!(normalize_path("/"), "/"); assert_eq!(normalize_path("/a/"), "/a"); }
    #[test] fn data_route_defaults_to_root() { assert_eq!(data_route_of("GET", "/__data", "/__data"), Some("/".into())); }
    #[test] fn data_route_decodes() { assert_eq!(data_route_of("GET", "/__data", "/__data?route=%2Fblog%2Fa"), Some("/blog/a".into())); }
    #[test] fn data_route_only_get() { assert_eq!(data_route_of("POST", "/__data", "/__data"), None); }
}
```

```rust
// crates/dispatch/src/flow.rs : the stage protocol used by A01 phase 3
pub enum Flow { Continue, Respond(oxide_server::OxideResponse) }
```

`chain.rs` (middleware chain, A05) and `loads.rs` (parallel load graph, A06) are added by those WPs. **Acceptance:** `cargo test -p oxide-dispatch` finishes in under 20 seconds on a cold cache after dependencies are built, and `cargo tree -p oxide-dispatch | grep -c jshost` prints 0.

### V2-A03 Hygiene bundle (S, no dependencies)

One commit per item; each has a proof command.

1. **CI branch filter.** `sed -n '3p' .github/workflows/ci.yml | od -c` must show `[main]`; fix if it shows a truncated token (HANDOFF wave2 §6). Proof: push a branch, the Rust matrix starts.
2. **Benchmark report.** `benchmarks/report.md` is stale (`Measured @ fa55c12`) and still feeds `benchmarks/gate.sh`. Re-run `benchmarks/run.sh`, `benchmarks/coldstart.sh`, `benchmarks/hmr.sh` (the `0/8` HMR figure predates SPEC-14), replace the table, record the commit hash. This is also V2-B09 step 1; do it once.
3. **`docs/architecture.md`.** Regenerate the crate table from `Cargo.toml` members (17 crates, plus `dispatch` after A02) and the pipeline diagram from the README. Proof: `grep -c "^| \`" docs/architecture.md\` equals the crate count.
4. **`WORKLOG.md` de-duplication.** Entries SPEC-09 and SPEC-17 appear twice, byte-identical. Run:

```python
# scripts/dedupe_worklog.py : drop byte-identical repeated sections of WORKLOG.md
import re, sys, hashlib
NL = chr(10)
text = open('WORKLOG.md').read()
parts = re.split(r'(?m)^(?=## )', text)
seen, out = set(), []
for p in parts:
    h = hashlib.sha256(p.strip().encode()).hexdigest()
    if p.startswith('## ') and h in seen:
        continue
    seen.add(h); out.append(p)
open('WORKLOG.md', 'w').write(''.join(out))
print('sections kept:', len(out), 'of', len(parts))
```

5. **Stale comments.** `crates/jshost/src/v8_ops.rs` header (claims ops are unregistered and cites a nonexistent `docs/deviations.md`); the middleware `{rewrite}` comment in the dispatch code (it now re-matches); the "IN PARALLEL" claim is already corrected and is rewritten again by A06.
6. **README.** Rewrite the "Built, wiring in progress" paragraph from the latest handoffs: router boot, `error.tsx` status, `await parent()`, `/__data` middleware and layout loads, PWA registration, HMR client injection. For each claim run the grep or the fixture that proves it; delete what is done. Update badge counts (crates, tests) from `cargo test` and `pnpm test` output.
7. **Commit policy check.** Add `scripts/commit-msg-check.sh` rejecting `feat(` on diffs that touch only whitespace (`git diff --cached -w --quiet`).

### V2-A04 Build id and generations substrate (M, depends on A01)

**Goal.** One canonical build id, visible to the browser and checked on every data-plane request. This is the keystone for F10 (skew), F11, F13, F14 and F20.

**Evidence.** `build_id` exists (`oxide_cache::build_id`, first 16 hex of blake3 over length-prefixed parts) and seeds the nonce and L0 keys; the HMR hub reads it from `dist/client/client.manifest.json`. No `x-oxide-build` header exists (`grep -rn x-oxide-build crates packages` is empty). `hydration_bootstrap` in `crates/render/src/lib.rs` emits `window.__oxide={route,data}` only.

**Design.**

1. **Definition.** `build_id = blake3(sorted lines 'relative/path<TAB>blake3-hex')` over every file of `dist/client/**`, `dist/server/**` and `.oxide/routes.manifest`, truncated to 16 hex. Written to `dist/build-id` (plain text) and `.oxide/build-id`. Computed at the end of `phase_emit` so it covers the final bytes. It replaces the current ad hoc value everywhere (`rg "build_id" crates` lists the call sites; each must read the one function `oxide_cache::build_id_of_dist(dist: &Path) -> Result<String>`).
2. **Browser.** `hydration_bootstrap` gains a fourth field: `window.__oxide={route:"...",build:"<id>",data:...}`. Update the signature to `hydration_bootstrap(odf_b64, route_id, nonce, build_id)` and every call site (compile errors list them).
3. **Server stage.** In `oxide-dispatch` add `build.rs`:

```rust
// crates/dispatch/src/build.rs
use std::time::{Duration, Instant};

pub const BUILD_HEADER: &str = "x-oxide-build";
pub const STALE_HEADER: &str = "x-oxide-build-stale";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen { Current, Previous }

/// The builds this process will answer data-plane requests for (F10 adds the pool side).
pub struct GenerationSet { current: String, previous: Option<(String, Instant)>, window: Duration }

impl GenerationSet {
    pub fn new(current: String, window: Duration) -> Self { Self { current, previous: None, window } }
    pub fn current(&self) -> &str { &self.current }
    /// Demote the current build to previous (called when a new generation is loaded).
    pub fn promote(&mut self, new_current: String, now: Instant) {
        let old = std::mem::replace(&mut self.current, new_current);
        self.previous = Some((old, now));
    }
    /// None means unknown or expired: the caller answers 409.
    pub fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        if id == self.current { return Some(Gen::Current); }
        match &self.previous {
            Some((p, since)) if p == id && now.duration_since(*since) <= self.window => Some(Gen::Previous),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn current_accepted() { let g = GenerationSet::new("a".into(), Duration::from_secs(60)); assert_eq!(g.accepts("a", Instant::now()), Some(Gen::Current)); }
    #[test] fn unknown_rejected() { let g = GenerationSet::new("a".into(), Duration::from_secs(60)); assert_eq!(g.accepts("zz", Instant::now()), None); }
    #[test] fn previous_accepted_inside_window_only() {
        let t0 = Instant::now();
        let mut g = GenerationSet::new("a".into(), Duration::from_secs(60));
        g.promote("b".into(), t0);
        assert_eq!(g.accepts("a", t0 + Duration::from_secs(30)), Some(Gen::Previous));
        assert_eq!(g.accepts("a", t0 + Duration::from_secs(61)), None);
    }
}
```

The dispatch stage `stage_build_check` runs only for the **data plane** (`/__data`, `/_oxide/fn/*`, later `/__oxide/live`, `/__oxide/stream/*`): if the request carries `x-oxide-build` and `accepts` returns `None`, respond `409` with `x-oxide-build-stale: true` and an empty body; if the header is absent (curl, tests, old clients) allow it. Every data-plane response carries `x-oxide-build: <current>`. 4. **Client.** New file `packages/oxide/src/client/build.ts` (core-owned, no dependencies):

```ts
// packages/oxide/src/client/build.ts
export function buildId(): string | undefined {
  return (globalThis as any).__oxide?.build
}

/** Add x-oxide-build to a fetch init; use for every data-plane request. */
export function withBuild(init: RequestInit = {}): RequestInit {
  const id = buildId()
  if (!id) return init
  const h = new Headers(init.headers)
  h.set('x-oxide-build', id)
  return { ...init, headers: h }
}

/** On 409 + x-oxide-build-stale: reload once per build id, never loop. */
export function handleStale(res: Response, reload: () => void = () => location.reload()): boolean {
  if (res.status !== 409 || res.headers.get('x-oxide-build-stale') !== 'true') return false
  const key = 'ox:reloaded:' + (buildId() ?? 'none')
  try {
    if (sessionStorage.getItem(key)) return false
    sessionStorage.setItem(key, '1')
  } catch { /* storage blocked: fall through and reload once */ }
  reload()
  return true
}
```

Use `withBuild` in `packages/oxide/src/client/router.ts` (the `/__data` fetch), `packages/oxide/src/fn.ts` (server-function stub) and `enhance.ts` (action fetch); call `handleStale` on the response before parsing.

**Tests.** Rust: the three `GenerationSet` tests above plus `build_id_of_dist_is_stable_and_order_independent` (create two temp trees with the same files in different creation order, equal ids; change one byte, different id). TS (`packages/oxide/test/build.test.ts`): `withBuild` adds the header only when `__oxide.build` exists; `handleStale` reloads once and the second call with the same id does not. Conformance: assertion `build id advertised` (HTML contains `build:` and `/__data` response carries `x-oxide-build`).

**Observable proof.** `oxide-cli build` twice with no source change prints the same `dist/build-id`; change one source byte and it differs; `curl -sI localhost:3000/__data?route=/ | grep -i x-oxide-build`.

### V2-A05 Full middleware chain and unified locals (M, depends on A02)

**Goal.** Run every ancestor `middleware.ts` outer to inner (§8.5), and make one `Locals` value reach loaders, layout loaders, actions, `route.ts` and render, including `/__data`.

**Evidence.** The dispatch code resolves only the *nearest* middleware (`best` by longest base). `MiddlewareDispatcher::dispatch(module_path, method, url, params, headers, body, locals)` already accepts the locals accumulated so far, and `MiddlewareRunResult { short, status, headers, body, redirect, rewrite, locals }` carries the directive. The i18n locale was found dropped in some paths (feature-designs §21.3).

**Design.** B01 provides `RouteContext.middleware_chain: Vec<String>` (module paths, outermost first). Implement the chain runner in `oxide-dispatch`:

```rust
// crates/dispatch/src/chain.rs
use oxide_server::{MiddlewareDispatcher, MiddlewareRunError, MiddlewareRunResult};
use serde_json::{Map, Value};

pub enum ChainOutcome {
    /// All middleware passed; carry merged locals and accumulated response headers.
    Continue { locals: Value, headers: Vec<(String, String)> },
    /// A middleware answered (short-circuit or redirect).
    Respond(MiddlewareRunResult),
    /// A middleware asked for a rewrite; the caller re-matches with the hop guard.
    Rewrite { to: String, locals: Value, headers: Vec<(String, String)> },
}

fn merge_locals(into: &mut Map<String, Value>, from: Value) {
    if let Value::Object(o) = from { for (k, v) in o { into.insert(k, v); } } // later wins
}

pub async fn run_chain(
    d: &dyn MiddlewareDispatcher,
    chain: &[String],
    method: &str, url: &str,
    params: &Map<String, Value>,
    headers: &[(String, String)],
    body: &[u8],
    initial_locals: Value,
) -> Result<ChainOutcome, MiddlewareRunError> {
    let mut locals = match initial_locals { Value::Object(o) => o, _ => Map::new() };
    let mut out_headers: Vec<(String, String)> = Vec::new();
    for path in chain {
        let cur = Value::Object(locals.clone());
        let r = d.dispatch(path, method, url, params, headers, body, Some(&cur)).await?;
        if let Some(l) = r.locals.clone() { merge_locals(&mut locals, l); }
        out_headers.extend(r.headers.iter().cloned());
        if r.short || r.redirect.is_some() { return Ok(ChainOutcome::Respond(r)); }
        if let Some(to) = r.rewrite.clone() {
            return Ok(ChainOutcome::Rewrite { to, locals: Value::Object(locals), headers: out_headers });
        }
    }
    Ok(ChainOutcome::Continue { locals: Value::Object(locals), headers: out_headers })
}
```

Tests (in the same file, using a fake dispatcher that implements the trait with a scripted `Vec<MiddlewareRunResult>` and records the `locals` argument it received): `runs_outer_to_inner`, `locals_merge_later_wins`, `inner_sees_outer_locals`, `short_circuit_stops_chain`, `redirect_stops_chain`, `rewrite_returns_target_with_locals`, `error_propagates`, `headers_accumulate_in_order`.

**Unified locals.** Add `crates/dispatch/src/locals.rs`:

```rust
#[derive(Default, Clone)]
pub struct RequestLocals { pub user: serde_json::Map<String, serde_json::Value>, pub session: Option<serde_json::Value>, pub locale: String, pub experiments: serde_json::Map<String, serde_json::Value> }

impl RequestLocals {
    /// The single JSON value handed to loaders, actions, endpoints and render.
    pub fn to_value(&self) -> serde_json::Value {
        let mut m = self.user.clone();
        if let Some(s) = &self.session { m.insert("session".into(), s.clone()); }
        if !self.locale.is_empty() { m.insert("locale".into(), self.locale.clone().into()); }
        if !self.experiments.is_empty() { m.insert("experiments".into(), serde_json::Value::Object(self.experiments.clone())); }
        serde_json::Value::Object(m)
    }
}
```

Reserved keys `session`, `locale`, `experiments` win over user-set keys with the same name (documented; a middleware that sets them gets a dev-mode warning `DSP-101`).

**Steps.** (1) Add `chain.rs`, `locals.rs`, tests, red then green. (2) In `stage_middleware` replace the nearest-ancestor scan with `run_chain(.., &route_ctx.middleware_chain, ..)`. (3) Run the same stage for `/__data` (today it skips middleware, README status) so a gated route cannot leak data through the data endpoint. (4) Build one `RequestLocals` in `stage_prelude` (session from the codec, locale from the i18n resolver) and pass `cx.locals.to_value()` to **every** consumer: `LoaderRunInput.locals`, the layout loop, `ActionDispatcher`, `EndpointDispatcher`, and the render input. Audit with `rg "locals" crates/cli/src/dispatch` and delete any `json!({})` literal standing in for locals.

**Acceptance.** Smoke app: add `routes/blog/middleware.ts` setting `locals.section = 'blog'` beside the root one; `/blog/x` loader sees both keys. A table-driven test over {page GET, `/__data`, action POST, `route.ts`} asserts the recording fake runner received identical locals. `curl /__data?route=/greeting?gated=1` redirects like the page does.

### V2-A06 Parallel layout loads (S, depends on A02, B01)

**Goal.** Meet the last explicit §9.2 MUST: layout and page loads run concurrently unless one calls `parent()`.

**Evidence.** The dispatch layout loop is sequential and its comment (near `lib.rs:4838-4852`) names the missing piece. `needs_parent` is computed only for pages, and crudely: `crates/router/src/scan.rs:175` uses `c.contains("parent")`; layouts always get `false`.

**Step 1: scanner.** Replace the `contains` test with a token test and apply it to layouts too:

```rust
// crates/router/src/scan.rs
/// True when the identifier `parent` appears as a whole token (over-approximation is safe:
/// a false positive only makes loads sequential).
fn calls_parent(src: &str) -> bool {
    let b = src.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let mut from = 0;
    while let Some(p) = src[from..].find("parent") {
        let s = from + p;
        let e = s + 6;
        let before = s == 0 || !ident(b[s - 1]);
        let after = e >= b.len() || !ident(b[e]);
        if before && after { return true; }
        from = e;
    }
    false
}
```

In the `needs_parent` computation use `calls_parent` on `page_server` for `Page` and on `layout_server` for `Layout`. Tests: `parent_token_detected`, `parentheses_not_enough` (`const parentDir = 1` is false), `layout_needs_parent_detected`. The rkyv archive field already exists (`archive.rs:39`), so bump `ARCHIVE_VERSION` if the crate has one, otherwise rebuild manifests.

**Step 2: load graph** in `oxide-dispatch`:

```rust
// crates/dispatch/src/loads.rs
use futures::future::{BoxFuture, FutureExt, Shared};
use oxide_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use std::sync::Arc;

pub struct LoadNode { pub input: LoaderRunInput, pub needs_parent: bool }

pub enum LoadResult {
    /// One output per node, in input order (outer layouts first, page last).
    Data(Vec<Arc<LoaderOutput>>),
    /// The first node (in order) that short-circuits (error() or redirect()).
    ShortCircuit(Arc<LoaderOutput>),
    Failed(String),
}

type Slot = Shared<BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<String>>>>;

pub async fn run_load_graph(runner: Arc<dyn LoaderRunner>, nodes: Vec<LoadNode>) -> LoadResult {
    let mut slots: Vec<Slot> = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.into_iter().enumerate() {
        let prev: Option<Slot> = if node.needs_parent && i > 0 { Some(slots[i - 1].clone()) } else { None };
        let runner = Arc::clone(&runner);
        let mut input = node.input;
        let fut: BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<String>>> = async move {
            if let Some(p) = prev {
                let parent = p.await?;
                input.parent_data = parent.data.clone();
            }
            runner.run(input).await.map(Arc::new).map_err(|e| Arc::new(e.0.message.clone()))
        }
        .boxed();
        slots.push(fut.shared());
    }
    let results = futures::future::join_all(slots).await;
    let mut outs = Vec::with_capacity(results.len());
    for r in results {
        match r {
            Err(m) => return LoadResult::Failed((*m).clone()),
            Ok(o) if o.short_circuits() => return LoadResult::ShortCircuit(o),
            Ok(o) => outs.push(o),
        }
    }
    LoadResult::Data(outs)
}
```

Semantics to document in the code comment and in the docs: independent loads may all run even if an outer one short-circuits; loads must therefore be idempotent and free of user-visible side effects (§9.2 already requires this of loaders). Result order, not completion order, decides which short-circuit wins, so behavior is deterministic.

Tests with a fake runner that sleeps: `tokio::test(start_paused = true)`; two independent nodes each sleeping 50 ms finish after 50 ms of virtual time (assert `elapsed < 60ms`); a `needs_parent` node starts only after its predecessor (assert start order and that `parent_data` equals the predecessor data); outer short-circuit wins over a failing inner; all failures map to `Failed`.

**Step 3: wire** `stage_loaders` to build `Vec<LoadNode>` from `route_ctx.layout_loads` plus the page, then apply side effects in order (cookies and headers accumulate outer to inner, exactly as today).

**Acceptance.** On a fixture with two independent 100 ms loads (layout and page) TTFB drops from about 200 ms to about 100 ms (`curl -w '%{time_starttransfer}'`); the existing `nested` smoke route (`await parent()` returns `acme-inc`) still passes.

### V2-A07 Prerender error-status sidecar (S, depends on A01)

**Goal.** A prerendered error page must not be served as 200.

**Design.** In `write_prerender`, when the render status is not 200, write `<route>/index.html.status` containing the decimal status. At serve start, scan `dist` once for `*.status` files into `StatusSidecars(HashMap<PathBuf, u16>)` (no per-request I/O) and have the static-asset responder apply the status. Code:

```rust
// crates/server/src/static_status.rs
pub struct StatusSidecars(std::collections::HashMap<std::path::PathBuf, u16>);
impl StatusSidecars {
    pub fn scan(root: &std::path::Path) -> Self {
        let mut m = std::collections::HashMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p); continue; }
                if p.extension().and_then(|x| x.to_str()) == Some("status") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        if let Ok(code) = s.trim().parse::<u16>() { m.insert(p.with_extension(""), code); }
                    }
                }
            }
        }
        Self(m)
    }
    pub fn status_for(&self, file: &std::path::Path) -> Option<u16> { self.0.get(file).copied() }
}
```

`with_extension("")` turns `index.html.status` into `index.html`. Tests: scan finds the sidecar; malformed content ignored; `status_for` returns `Some(500)`. Proof: build the smoke app with a prerenderable `routes/boom` page; `curl -sI` the built file route returns `500`, and the L0 seed path stores the same status.

### V2-A08 V8 backend parity for the newer ops (M, no dependencies)

**Goal.** The V8 host handles every op the Node worker handles, proven by one shared test table.

**Evidence.** HANDOFF bucket2 "What remains" item 3: `endpoint:run`, `middleware:run`, deferred awaiting and `renderStream` are Node-worker only; `v8_dispatch` (grep `fn v8_dispatch` in `crates/jshost/src/lib.rs`) has a hand-ported subset.

**Steps.**

1. List worker kinds: `grep -n "case '" crates/jshost/worker/oxide-worker.mjs`. List V8 kinds: the match arms of `v8_dispatch`. The difference is the work list.
2. For each missing kind add one arm that calls the same JS entry through the V8 module loader (`v8_loader.rs`) and returns the same JSON shape. Do not fork semantics: where the worker does work in JS, load that same JS in the isolate.
3. Add `crates/jshost/tests/op_parity.rs` (`#[ignore]`, engine dependent; run with `--ignored`):

```rust
// one fixture module per op; the same inputs go to both hosts and outputs are compared as JSON
const CASES: &[(&str, &str)] = &[
    ("endpoint:run", "fixtures/ops/endpoint_get.json"),
    ("middleware:run", "fixtures/ops/middleware_locals.json"),
    ("loader:run", "fixtures/ops/loader_defer.json"),
    ("action:run", "fixtures/ops/action_basic.json"),
];
#[tokio::test] #[ignore]
async fn node_and_v8_agree() {
    for (kind, file) in CASES {
        let input: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let a = call(node_host().await, kind, input.clone()).await;
        let b = call(v8_host().await, kind, input).await;
        assert_eq!(normalize(a), normalize(b), "op {kind} diverges");
    }
}
```

`normalize` sorts object keys and drops timing fields. 4. Extend `tests/fixtures/check-v8.sh` with endpoint, middleware and streaming cases on the smoke app.

**Acceptance.** `cargo test -p oxide-jshost --ignored op_parity` green; `check-v8.sh` passes `/api/users` GET and POST, `/greeting` locals, and `/slow` streaming; capability row `Snapshots` unaffected.

### V2-A09 Plugin transforms on the prerender path (S, depends on A01)

**Goal.** Prerendered HTML reflects plugin-transformed code (SPEC-17 known remainder).

**Cause.** Prerender renders through the page renderer's module loader, which loads **source** modules, not the plugin-transformed server bundle.

**Design.** Add `ModuleSource { Source, Bundle(PathBuf) }` to the framework renderer's config. `phase_prerender` runs after `phase_bundle_server`, so it constructs the renderer with `ModuleSource::Bundle(dist/server)`. Dev and `oxide render` keep `Source`. Steps: find where prerender builds its `PageRenderer` (`grep -n "PageRenderer" crates/cli/src/prerender.rs crates/cli/src/build`), thread `ModuleSource`, and make the worker's `real-ssr` loader import from the bundle path when given. **Proof:** the SPEC-17 fixture (plugin rewrites `PLUGIN_MARKER` to `TRANSFORMED_BY_PLUGIN`) must now show the transformed string in `dist/**/index.html`; add that assertion to the fixture script. **Risk:** bundled server chunks may inline framework code that the worker also resolves; if a duplicate React instance appears (hooks error), mark framework packages external in the SSR bundle options exactly as the live path does.

### V2-A10 Redis and S3 cache handlers (M, no dependencies, feature-gated)

**Goal.** Replace the loud no-op in the `cache.handler` match arm (`crates/cli/src/serve/start.rs`, formerly `lib.rs:6584`) with real remote handlers.

**Design.**

- Cargo features on `oxide-cache`: `redis` (crate `redis` with `tokio-comp` and `connection-manager`) and `s3` (crate `object_store` with `aws`). Add both crates to the root `[workspace.dependencies]` pinned to the newest versions that build (run `cargo add --dry-run` first; do not guess versions). `oxide-cli` forwards `cache-redis` and `cache-s3` features; the release binary enables both.
- Config (schema keys, `camelCase`): `cache.handler: { type: 'redis', url, prefix? }` and `{ type: 's3', bucket, prefix?, region?, endpoint? }`; secrets come from the URL or the standard AWS env.
- Redis key layout: entry `oxide:{prefix}:e:{key}` (zstd of JSON `Entry`, with `PX` equal to the entry's stale-until horizon); tag set `oxide:{prefix}:t:{tag}` (`SADD`); invalidation channel `oxide:{prefix}:inv`.

```rust
// crates/cache/src/redis_handler.rs  (#[cfg(feature = "redis")])
use crate::{CacheHandler, Entry, Result, err};
use redis::AsyncCommands;

pub struct RedisHandler { cm: redis::aio::ConnectionManager, prefix: String }

impl RedisHandler {
    pub async fn connect(url: &str, prefix: &str) -> Result<Self> {
        let c = redis::Client::open(url).map_err(|e| err("CAC-REDIS", format!("open: {e}")))?;
        let cm = c.get_connection_manager().await.map_err(|e| err("CAC-REDIS", format!("connect: {e}")))?;
        Ok(Self { cm, prefix: prefix.to_string() })
    }
    fn ek(&self, k: &str) -> String { format!("oxide:{}:e:{}", self.prefix, k) }
    fn tk(&self, t: &str) -> String { format!("oxide:{}:t:{}", self.prefix, t) }
    fn chan(&self) -> String { format!("oxide:{}:inv", self.prefix) }
}

#[async_trait::async_trait]
impl CacheHandler for RedisHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        let mut c = self.cm.clone();
        let raw: Option<Vec<u8>> = c.get(self.ek(key)).await.map_err(|e| err("CAC-REDIS", e.to_string()))?;
        Ok(match raw { Some(b) => serde_json::from_slice(&zstd::decode_all(&b[..]).map_err(|e| err("CAC-ZSTD", e.to_string()))?).ok(), None => None })
    }
    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        let mut c = self.cm.clone();
        let json = serde_json::to_vec(&entry).map_err(|e| err("CAC-JSON", e.to_string()))?;
        let z = zstd::encode_all(&json[..], 3).map_err(|e| err("CAC-ZSTD", e.to_string()))?;
        let ttl_ms = entry.stale_until_ms_from_now().max(1000);   // add this helper next to Entry::fresh_until
        let mut pipe = redis::pipe();
        pipe.cmd("SET").arg(self.ek(key)).arg(z).arg("PX").arg(ttl_ms).ignore();
        for t in &entry.tags { pipe.cmd("SADD").arg(self.tk(t)).arg(key).ignore(); }
        pipe.query_async::<()>(&mut c).await.map_err(|e| err("CAC-REDIS", e.to_string()))
    }
    async fn delete(&self, key: &str) -> Result<()> {
        let mut c = self.cm.clone();
        c.del::<_, ()>(self.ek(key)).await.map_err(|e| err("CAC-REDIS", e.to_string()))
    }
    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let mut c = self.cm.clone();
        c.smembers(self.tk(tag)).await.map_err(|e| err("CAC-REDIS", e.to_string()))
    }
    async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize> {
        let keys = self.tag_keys(tag).await?;
        let mut c = self.cm.clone();
        if !soft { for k in &keys { let _: () = c.del(self.ek(k)).await.map_err(|e| err("CAC-REDIS", e.to_string()))?; } }
        let _: () = c.del(self.tk(tag)).await.map_err(|e| err("CAC-REDIS", e.to_string()))?;
        Ok(keys.len())
    }
    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()> {
        let mut c = self.cm.clone();
        let msg = format!("{}:{}", if soft { "s" } else { "h" }, tag);
        c.publish::<_, _, ()>(self.chan(), msg).await.map_err(|e| err("CAC-REDIS", e.to_string()))
    }
}
```

- **Subscriber.** At server start, when the handler is Redis, spawn a task that `SUBSCRIBE`s to the channel and calls the local `TieredCache::invalidate_tag_local(tag, soft)` (add this method: it clears memory and disk tiers only and does **not** call `publish_invalidation`, otherwise nodes echo forever). Add a node id prefix to the message and ignore your own.
- **S3 handler** (`s3_handler.rs`): entries at `{prefix}/e/{hash[..2]}/{hash}` (zstd JSON); tag membership as empty marker objects `{prefix}/t/{tag}/{keyhash}`; `invalidate_tag` lists the tag prefix and deletes; `publish_invalidation` is a no-op (document: S3 has no pub/sub, so multi-node coherence relies on `Cache-Control` TTLs and the SWR window).
- **Tests.** Handler contract suite (`crates/cache/tests/handler_contract.rs`) generic over `Box<dyn CacheHandler>`: set/get round trip, TTL expiry, tag invalidate hard and soft, delete. Run it against the in-memory handler always, against Redis only when `OXIDE_TEST_REDIS_URL` is set, against S3 only when `OXIDE_TEST_S3_ENDPOINT` is set (MinIO). Skipped tests print `SKIP`, never silently pass.
- **Capability matrix.** Redis and S3 rows are `Full` on Binary, Node and Docker; `Degraded` on serverless (cold connection); surface in `oxide doctor` (`cache handler reachable`).

**Acceptance.** Two `oxide start` processes sharing a Redis: `revalidate('posts')` on node A makes node B serve a fresh render for a tagged route within 100 ms (curl both).

## V2.6 Wave B: performance

Rules for every B WP: take the **before** number first (B09 lands first), keep behavior identical, and prove equivalence with the old implementation as a test oracle until the new one has shipped for a release.

### V2-B01 `RouteContextTable` (S, depends on A02)

**Goal.** Every answer the dispatcher needs that is a pure function of the matched route id is computed once at boot.

**Evidence.** Per request the dispatch closure scans `tree.entries` for middleware, error and layout modules, and runs `.filter(|(_, p)| p.is_file())` on each layout module, which is a filesystem `stat` syscall per layout per request. The prerender loop repeats the same scans per route (cli `3294-3330`). `layout_chain()` in `router/src/matcher.rs:156` is itself O(entries) and `arch_match_impl` calls it on every match.

**Files.** Create `crates/router/src/context.rs`; export from `crates/router/src/lib.rs` (`mod context; pub use context::{LayoutLoad, RouteContext, RouteContextTable};`). Consumers: `dispatch`, `prerender`.

```rust
// crates/router/src/context.rs
use crate::{RouteEntry, RouteId, RouteKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutLoad { pub id: RouteId, pub server_file: PathBuf, pub needs_parent: bool }

#[derive(Debug, Clone, PartialEq)]
pub struct RouteContext {
    pub route_id: RouteId,
    pub pattern: Arc<str>,                 // precomputed metrics label (replaces per-request pattern_of)
    pub layout_chain: Vec<RouteId>,        // outer to inner, same order as matcher::layout_chain
    pub layout_loads: Vec<LayoutLoad>,     // layouts that have layout.server.ts (flag, no stat call)
    pub middleware_chain: Vec<PathBuf>,    // module files, outer to inner (A05)
    pub error_route: Option<RouteId>,      // innermost error.tsx
    pub loading_route: Option<RouteId>,    // innermost loading.tsx
}

#[derive(Default)]
struct Scoped<'a> { layout: Option<&'a RouteEntry>, middleware: Option<&'a RouteEntry>, error: Option<&'a RouteEntry>, loading: Option<&'a RouteEntry> }

pub struct RouteContextTable { by_id: HashMap<String, Arc<RouteContext>>, unmatched: Option<Arc<RouteContext>> }

/// Directory prefixes of a route id at path boundaries, outermost first, always starting with the
/// empty base: `routes/blog/page` gives `""`, `routes`, `routes/blog`, `routes/blog/page`.
fn prefixes(id: &str) -> Vec<&str> {
    let mut v = vec![""];
    for (i, c) in id.char_indices() { if c == '/' { v.push(&id[..i]); } }
    v.push(id);
    v
}

fn base<'a>(id: &'a str, suffix: &str) -> &'a str { id.strip_suffix(suffix).unwrap_or("") }

impl RouteContextTable {
    pub fn build(entries: &[RouteEntry], pattern_of: impl Fn(&RouteEntry) -> String) -> Self {
        let mut sorted: Vec<&RouteEntry> = entries.iter().collect();
        sorted.sort_by(|a, b| a.id.0.cmp(&b.id.0));          // determinism: first id wins a tie
        let mut scoped: HashMap<&str, Scoped> = HashMap::new();
        for e in &sorted {
            let (b, slot): (&str, fn(&mut Scoped<'_>) -> &mut Option<&RouteEntry>) = match e.kind {
                RouteKind::Layout => (base(&e.id.0, "/layout"), |s| &mut s.layout),
                RouteKind::Middleware => (base(&e.id.0, "/middleware"), |s| &mut s.middleware),
                RouteKind::Error => (base(&e.id.0, "/error"), |s| &mut s.error),
                RouteKind::Loading => (base(&e.id.0, "/loading"), |s| &mut s.loading),
                _ => continue,
            };
            let s = scoped.entry(b).or_default();
            let cell = slot(s);
            if cell.is_none() { *cell = Some(e); }
        }
        let mut by_id = HashMap::with_capacity(entries.len());
        for e in entries.iter().filter(|e| !matches!(e.kind, RouteKind::Layout | RouteKind::Middleware | RouteKind::Error | RouteKind::Loading)) {
            by_id.insert(e.id.0.clone(), Arc::new(Self::context_for(e, &scoped, &pattern_of)));
        }
        let unmatched = entries.iter().find(|e| matches!(e.kind, RouteKind::NotFound))
            .map(|e| Arc::new(Self::context_for(e, &scoped, &pattern_of)));
        Self { by_id, unmatched }
    }

    fn context_for(e: &RouteEntry, scoped: &HashMap<&str, Scoped<'_>>, pattern_of: &impl Fn(&RouteEntry) -> String) -> RouteContext {
        let mut ctx = RouteContext {
            route_id: e.id.clone(), pattern: pattern_of(e).into(), layout_chain: vec![], layout_loads: vec![],
            middleware_chain: vec![], error_route: None, loading_route: None,
        };
        for p in prefixes(&e.id.0) {
            let Some(s) = scoped.get(p) else { continue };
            if let Some(l) = s.layout {
                ctx.layout_chain.push(l.id.clone());
                if l.has_server_module {
                    ctx.layout_loads.push(LayoutLoad { id: l.id.clone(), server_file: l.file.with_file_name("layout.server.ts"), needs_parent: l.needs_parent });
                }
            }
            if let Some(m) = s.middleware { ctx.middleware_chain.push(m.file.clone()); }
            if let Some(x) = s.error { ctx.error_route = Some(x.id.clone()); }     // inner overwrites outer
            if let Some(x) = s.loading { ctx.loading_route = Some(x.id.clone()); }
        }
        ctx
    }

    pub fn get(&self, route_id: &str) -> Option<&Arc<RouteContext>> { self.by_id.get(route_id) }
    pub fn unmatched(&self) -> Option<&Arc<RouteContext>> { self.unmatched.as_ref() }
}
```

**Semantics to preserve.** The old code treated base `routes` and the empty base as "wraps everything"; `prefixes` always includes `""` and `routes`, so both match. The old `layout_chain` sorted by base length and tie-broke on id; `prefixes` is already outermost-first and the map keeps one layout per base (the lexicographically first id). Not-found: the old code used the **first** `NotFound` entry in `tree.entries` order; keep that (`entries.iter().find`), not the sorted order.

**Tests (`crates/router/src/context.rs`, red first).**

1. `matches_layout_chain_oracle`: for every fixture tree (the existing `archive.rs` fixture plus a nested-groups fixture), assert `table.get(id).layout_chain == matcher::layout_chain(&entries, id)` for every non-layout entry. This is the equivalence proof.
2. `inner_error_shadows_outer`, `middleware_chain_outer_to_inner`, `layout_loads_only_with_server_module`, `layout_loads_carry_needs_parent`.
3. `unmatched_uses_first_not_found`.
4. A `proptest` generating random directory trees with layouts, middleware and errors at random depths, asserting the oracle equality.

**Integration.** Build the table once where the route tree is loaded (`start_server_inner_full` before `build_dispatch`), store `Arc<RouteContextTable>` in `DispatchDeps`, and in `stage_match` do `let rc = deps.ctx.get(&m.route.id.0)`. Delete the four scans and the `is_file()` filter. Replace the prerender loop scans (`3294-3330`) with the same table. `pattern_of` is the existing function (pass it as the closure).

**Acceptance.** `rg "for candidate in &tree.entries" crates/cli/src` returns nothing; `strace -c` (or `dtruss -c` on macOS) on 1,000 requests to a route with two layouts shows zero `stat`/`fstat` calls attributable to layout modules; the smoke-app digests (A01 Step 0) are unchanged.

### V2-B02 Candidate index and allocation-free matcher (M, no dependencies)

**Goal.** Route matching cost depends on URL depth, not on route count, and the static path allocates nothing.

**Evidence.** `match_in` (`router/src/matcher.rs:60`) loops over every entry, allocates `acc: Vec<(String, String)>` per entry, `prios: Vec<u8>` per matching entry, `url: Vec<String>` from `split_path`, and `MatchResult.route` is a cloned `RouteEntry`. The spec (§8.2) claims allocation-free matching.

**Decision (D23).** Do not rewrite scoring. Add a **candidate index** that returns, in original order, a superset of the entries that can match, then run the existing exact scorer on only those. Correctness is by construction: an entry is omitted only when its static prefix provably differs from the URL.

```rust
// crates/router/src/index.rs
use crate::matcher::{EntryLike, SegKind, classify};
use smallvec::SmallVec;
use std::collections::HashMap;

#[derive(Default)]
struct Node { stat: HashMap<Box<str>, u32>, wild: Vec<u32>, term: Vec<u32> }

/// stat: static-segment children. wild: entries whose next segment is dynamic (param, optional,
/// catch-all) and whose preceding segments are all static and lead to this node.
/// term: entries made only of static segments that end exactly here.
pub struct CandidateIndex { nodes: Vec<Node> }

impl CandidateIndex {
    pub fn build<E: EntryLike>(entries: &[E]) -> Self {
        let mut nodes = vec![Node::default()];
        for (i, e) in entries.iter().enumerate() {
            if !e.matchable() { continue; }
            let mut cur = 0usize;
            let mut placed = false;
            for k in 0..e.seg_len() {
                match classify(e.seg(k)) {
                    SegKind::Static(s) => {
                        let next = match nodes[cur].stat.get(s) {
                            Some(&n) => n as usize,
                            None => { nodes.push(Node::default()); let n = nodes.len() - 1; nodes[cur].stat.insert(s.into(), n as u32); n }
                        };
                        cur = next;
                    }
                    _ => { nodes[cur].wild.push(i as u32); placed = true; break; }
                }
            }
            if !placed { nodes[cur].term.push(i as u32); }
        }
        Self { nodes }
    }

    /// Candidate entry indices for these URL segments, ascending (original order).
    pub fn candidates(&self, url: &[&str], out: &mut SmallVec<[u32; 16]>) {
        let (mut cur, mut d) = (0usize, 0usize);
        loop {
            out.extend_from_slice(&self.nodes[cur].wild);          // dynamic entries reachable here
            if d == url.len() { out.extend_from_slice(&self.nodes[cur].term); break; }
            match self.nodes[cur].stat.get(url[d]) { Some(&n) => { cur = n as usize; d += 1; } None => break }
        }
        out.sort_unstable();
    }
}
```

`wild` is also consulted at `d == url.len()`, which is what lets `[[...optional]]` match zero segments.

**Allocation-free scorer.** Make these changes in `matcher.rs` (keep the old function as `match_in_linear`, `#[cfg(test)]`, as the oracle):

1. `split_path` returns `SmallVec<[&str; 8]>` borrowing from the path instead of `Vec<String>`; `match_entry` takes `url: &[&str]`.
2. Captures: `acc: &mut SmallVec<[(&str, Capture<'_>); 4]>` with `enum Capture<'a> { Seg(&'a str), Joined(String) }`. Only catch-all captures allocate (the join).
3. Replace the `prios: Vec<u8>` per candidate with a lazily compared iterator: keep `best` as `(idx, score)` and compute `seg_priority` comparisons by index (`(0..len).map(|k| seg_priority(e.seg(k)))` compared with `Iterator::cmp`), so no vector is built.
4. New API `RouteTree::match_ref(&self, path) -> Option<MatchRef<'_>>` where `MatchRef { route: &RouteEntry, params: SmallVec<[(Cow<str>, Cow<str>); 4]>, score: i64 }`. Keep `match_path` (it calls `match_ref` and clones) so no caller breaks.
5. Archived tree: materialize `Arc<RouteEntry>` once per entry at load (`OnceLock<Vec<Arc<RouteEntry>>>` inside `ArchivedTree`) and add `ArchivedTree::match_ref -> Option<(Arc<RouteEntry>, SmallVec<..>)>` so the hot path clones an `Arc`, not strings. `arch_match_impl` stops calling `layout_chain` (B01 supplies it).
6. Both `RouteTree` and `ArchivedTree` build the `CandidateIndex` once (archived: at load from the archived entries; it is cheap and not part of the rkyv archive).

Add `smallvec = "1"` to `[workspace.dependencies]` (pin the exact current 1.x from `cargo add --dry-run smallvec`) and to `crates/router/Cargo.toml`.

**Tests (red first).**

- `index_candidates_superset`: for every fixture and every URL in a generated list, the linear winner index is contained in `candidates()`.
- **Oracle property test** (`proptest`, in `crates/router/tests/index_oracle.rs`): random route sets over the segment alphabet `[a, b, c, [x], [x:[0-9]+], [...r], [[...o]]]` with depth up to 4 (30 routes max) and random URLs of depth 0 to 5: `match_ref` result equals `match_in_linear` (same entry index, same params, same score). Run 10,000 cases in CI and 200,000 nightly. Also run the existing `fuzz/fuzz_targets/router_match.rs` against the new matcher.
- `static_route_makes_no_heap_allocation`: with `dhat` testing mode (see B09) assert zero blocks for matching `/blog` on a 1,000-route tree.

**Acceptance.** Criterion `router/match/1000_routes/static` at least 10x faster than the B09 baseline; all existing router tests (55) and the oracle property test green; `cargo fuzz run router_match -- -max_total_time=60` clean.

### V2-B03 Hot-path hoists (S, depends on A01)

Four small changes in the dispatch stages (they are listed together because each is a few lines and each has its own commit).

1. **Endpoint inputs only when needed.** Move `endpoint_params_json`, `endpoint_headers`, `endpoint_body`, `endpoint_method_for_dispatch`, `endpoint_url_for_dispatch` (about `lib.rs:4332-4336`) into `stage_endpoint`, inside the `if let (Some(dispatcher), Some(mp))` branch they serve. Same for `action_params`, which is built for every request though only POST actions use it. Proof: a counting allocator test (B09) on a page GET shows at least 5 fewer allocations per request.
2. **Metrics label.** Delete the per-request `pattern_of(&m.route.segments)`; use `rc.pattern` (`Arc<str>`, B01). For unmatched use a `static UNMATCHED: &str`.
3. **Nonce once.** `nonce_for` is called at five sites. Add a lazily computed field:

```rust
// dispatch/ctx.rs
pub(crate) struct RequestCtx { /* ... */ nonce: std::cell::OnceCell<std::sync::Arc<str>> }
impl RequestCtx {
    pub(crate) fn nonce(&self) -> std::sync::Arc<str> {
        self.nonce.get_or_init(|| oxide_server::nonce_for(&self.req.request_id).into()).clone()
    }
}
```

Replace each `oxide_server::nonce_for(&req_owned.request_id)` with `cx.nonce()`. Equality test: `nonce_is_stable_within_a_request`. 4. **Metrics without a write lock.** `Metrics::observe` (`metrics.rs:88`) takes an `RwLock` read per request and a write lock on first sight of a label. Add `Metrics::register_routes(labels: impl IntoIterator<Item = Arc<str>>)` that fills a frozen `HashMap<Arc<str>, RouteMetrics>` (built before the server starts and then immutable, so lookups need no lock), keeping the `RwLock` map only as the fallback for dynamic labels (`_unmatched`, `_proxy`, `_data`). `RouteMetrics::observe` already uses atomics. Call `register_routes` from `start_server_inner_full` with every `RouteContext.pattern`. Test: `observe_registered_route_takes_no_lock` (hold the fallback write lock in another thread and assert `observe` on a registered label still returns).

**Acceptance.** Allocation count per page GET drops by the amount in B09's table; `rg "nonce_for" crates/cli/src/dispatch` shows one definition site.

### V2-B04 Share the request instead of cloning it (S, depends on A01)

**Evidence.** `let req_owned = req.clone();` (about `lib.rs:4752`) clones the header `Vec<(String, String)>` and body handle per request; `OxideRequest` is `Clone`.

**Change.** Do not change `OxideRequest`'s public shape (it is constructed by tests). Inside the dispatch closure take ownership once: `let req = std::sync::Arc::new(req);` at the top of the closure, pass `Arc<OxideRequest>` into stages (`cx.req: Arc<OxideRequest>`), and delete the `req.clone()` calls. Stages that need owned headers for a runner input call `cx.req.headers.clone()` only inside the branch that builds that input. Test: `dispatch_does_not_clone_request` using a request whose header vector length is large and a counting global allocator (B09) comparing allocation counts for 1 and 100 headers (must be equal within 2). **Acceptance:** `rg "req.clone()" crates/cli/src/dispatch` is empty except inside proxy forwarding.

### V2-B05 Heap-free cache keys and an O(1) tag index (S, no dependencies)

**Evidence.** `CacheKey::canonical()` (`cache/src/lib.rs:113`) builds a 64-char hex `String` per lookup; `L0Cache::key(route, params, vary)` does the same. In `TieredCache::set` (`cache/src/tiers.rs` about 521-535) the tag index is `DashMap<String, Vec<String>>` with `if !list.iter().any(|k| k == key)`: O(n) per insert, so an entry with 5 tags under a tag that holds 10,000 keys costs 50,000 string compares. `delete` leaves dangling keys that are cleaned "lazily on the next tag invalidation".

**Keys without heap allocation (no API break).** The `CacheHandler` trait is string-keyed, so keep strings at the trait boundary but build them on the stack:

```rust
// crates/cache/src/keyhash.rs
/// 32 ASCII hex chars (a 128-bit blake3 prefix) on the stack. Derefs to &str.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyStr([u8; 32]);

impl KeyStr {
    pub fn as_str(&self) -> &str { std::str::from_utf8(&self.0).expect("hex is ascii") }
}
impl std::ops::Deref for KeyStr { type Target = str; fn deref(&self) -> &str { self.as_str() } }

pub fn key_of(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> KeyStr {
    let mut h = blake3::Hasher::new();
    for (i, p) in [layer, route, params, vary, build_id].iter().enumerate() {
        if i > 0 { h.update(&[0]); }          // NUL separators keep the encoding prefix-free
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 32];
    for (i, b) in d.as_bytes()[..16].iter().enumerate() {
        out[2 * i] = HEX[(b >> 4) as usize];
        out[2 * i + 1] = HEX[(b & 15) as usize];
    }
    KeyStr(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deterministic() { assert_eq!(&*key_of("L0", "r", "p", "v", "b"), &*key_of("L0", "r", "p", "v", "b")); }
    #[test] fn prefix_free() { assert_ne!(&*key_of("L0", "ab", "c", "", ""), &*key_of("L0", "a", "bc", "", "")); }
    #[test] fn matches_canonical_prefix() {
        // canonical() hashes the same byte sequence, so its first 32 hex chars equal key_of.
        let k = crate::CacheKey { layer: crate::Layer::L0, route: "r".into(), params: "p".into(), vary: "v".into(), build_id: "b".into() };
        assert_eq!(&k.canonical()[..32], &*key_of("L0", "r", "p", "v", "b"));
    }
}
```

`canonical()` stays (disk blob names and existing tests use it). `L0Cache::key` becomes `fn key(&self, ..) -> KeyStr` and callers pass `&key` (deref gives `&str`). **Warning:** switching L0 keys from 16 to 32 hex chars changes every stored key; that is fine because the build id is part of the key and L0 is rebuilt per build, but run `oxide cache clear` in the migration notes. If you must keep the old 16-char form, truncate in `key_of`'s last step and drop the `matches_canonical_prefix` length to 16.

**Tag index.** In `TieredCache`:

```rust
// before: tags: DashMap<String, Vec<String>>
// after:
tags: dashmap::DashMap<String, std::collections::HashSet<String>>,       // tag -> keys
key_tags: dashmap::DashMap<String, Vec<String>>,                          // key -> tags (reverse index)

pub async fn set(&self, key: &str, entry: Entry) -> Result<()> {
    self.memory.insert(key, entry.clone());
    if let Some(disk) = &self.disk { disk.set(key, &entry)?; }
    if let Some(remote) = &self.remote { remote.set(key, entry.clone()).await?; }
    for tag in &entry.tags {
        self.tags.entry(tag.clone()).or_default().insert(key.to_string());   // O(1), idempotent
    }
    self.key_tags.insert(key.to_string(), entry.tags.clone());
    Ok(())
}

pub async fn delete(&self, key: &str) -> Result<()> {
    self.memory.remove(key);
    if let Some(disk) = &self.disk { disk.delete(key)?; }
    if let Some(remote) = &self.remote { remote.delete(key).await?; }
    if let Some((_, tags)) = self.key_tags.remove(key) {             // eager cleanup, no dangling keys
        for t in tags { if let Some(mut s) = self.tags.get_mut(&t) { s.remove(key); } }
    }
    Ok(())
}
```

Fix the remaining uses (`tag_keys`, `invalidate_tag`, `invalidate_path`) by collecting the set into a `Vec` where the trait demands one. A re-`set` of a key whose tag list changed must remove it from tags it no longer has: before inserting, `if let Some(old) = self.key_tags.get(key) { for t in old.iter().filter(|t| !entry.tags.contains(t)) { remove } }`.

**Tests (red first).** `tag_insert_is_idempotent` (insert the same key 3 times, `tag_keys` length 1); `delete_cleans_tag_sets`; `retag_removes_stale_membership`; `invalidate_tag_deletes_all_keys` (existing); a timing guard `tag_insert_scales` that inserts 20,000 keys under one tag and asserts total time under 250 ms in release mode (`#[ignore]` in debug). Criterion `cache/tag_insert/10k` for B09.

### V2-B06 Disk-tier tags as a redb multimap (S, depends on B05)

**Evidence.** `DiskTier::set` (`tiers.rs` about 209-252) serializes the whole entry to JSON, then for each tag does read, `serde_json::from_slice`, linear dedupe, `serde_json::to_vec`, write: a JSON round trip of the whole tag list per tag per cache write, all inside one write transaction.

**Change.** Use a redb multimap table so each tag membership is one insert:

```rust
use redb::{MultimapTableDefinition, ReadableMultimapTable};
const TAG_KEYS: MultimapTableDefinition<&str, &str> = MultimapTableDefinition::new("tag_keys_v2");

// in DiskTier::set, replacing the JSON read-modify-write loop:
let mut mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
for tag in &entry.tags { mm.insert(tag.as_str(), key).map_err(dberr)?; }     // idempotent set semantics

// tag_keys:
let txn = self.index.begin_read().map_err(dberr)?;
let mm = txn.open_multimap_table(TAG_KEYS).map_err(dberr)?;
let mut out = Vec::new();
for v in mm.get(tag).map_err(dberr)? { out.push(v.map_err(dberr)?.value().to_string()); }

// delete(key): look up the entry's tags from its index row (DiskIndexEntry.tags) and
// mm.remove(tag, key) for each; invalidate_tag: iterate then mm.remove_all(tag).
```

**Migration.** On open, if the old `TAG_TABLE` has rows: in one write transaction read each row (`tag -> JSON list`), insert every pair into `TAG_KEYS`, then delete the old rows; record `schema=2` in a small meta table so it runs once. Test `migrates_v1_tag_table`: create a v1 database fixture with the old table (build it in the test using the old table definition), open with the new code, assert `tag_keys` returns the same keys and the old table is empty. If the redb 4.3 multimap method names differ from the above, adapt them from `cargo doc -p redb --open`; the semantics (set-valued key) are what matters.

**Acceptance.** Criterion `cache/disk_set_5_tags` at least 3x faster than the B09 baseline; the existing disk-tier tests pass unchanged.

### V2-B07 Parallel precompile (S, no dependencies)

**Evidence.** `precompile_entries` (`cli/src/precompile.rs:230`) is `while let Some(path) = queue.pop_front()` and awaits each compile in turn, so the tooling pool's workers sit idle.

**Design.** Two phases: a cheap synchronous discovery walk that finds every file needing a compile, then a bounded-concurrency compile of the independent files. Discovery already only needs the import scanner.

```rust
// precompile.rs (sketch of the new body; keep the existing helper functions)
use futures::stream::{self, StreamExt};

pub async fn precompile_entries(pool: Option<&Arc<dyn Pool>>, entries: &BTreeMap<String, PathBuf>,
    svelte_generate: &str, framework: Option<&str>, cache: Option<&PrecompileCache>, tool_version: &str)
    -> BTreeMap<PathBuf, String>
{
    let Some(pool) = pool else { return BTreeMap::new() };
    // Phase 1: discovery (sync, no pool calls)
    let mut jobs: Vec<(PathBuf, &'static str, String)> = Vec::new();     // (path, kind, source)
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: VecDeque<PathBuf> = entries.values().cloned().collect();
    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) { continue; }
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        for spec in scan_import_specs(&source) { if let Some(dep) = resolve_relative(&path, &spec) { queue.push_back(dep); } }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let kind = match ext {
            "tsx" | "jsx" if framework == Some("solid") => "solid",
            "vue" => "vue",
            "svelte" => "svelte",
            _ => continue,
        };
        jobs.push((path, kind, source));
    }
    // Phase 2: compile with bounded concurrency; stats().workers sizes the window
    let width = (pool.stats().workers.max(1)) * 2;
    let results: Vec<(PathBuf, anyhow::Result<String>)> = stream::iter(jobs)
        .map(|(path, kind, source)| async move {
            let key = cache_key_for(&path, &source, kind, svelte_generate, tool_version);
            let r = cached_compile(cache, key, || async { compile_one(pool, kind, &path, &source, svelte_generate).await }).await;
            (path, r)
        })
        .buffer_unordered(width)
        .collect().await;
    let mut out = BTreeMap::new();                       // BTreeMap: output order is deterministic
    for (path, r) in results {
        match r { Ok(js) => { out.insert(path, js); } Err(e) => eprintln!("warn: {} precompile failed for {}: {e}", "framework", path.display()) }
    }
    out
}
```

`compile_one` is a small `match kind` over the existing `compile_solid`, `compile_vue`, `compile_svelte`. Native compilers (C-series) plug in at `compile_one` and run on `spawn_blocking`, with `width = available_parallelism()`.

**Tests.** `precompile_is_deterministic` (run twice, equal maps, with a fake pool that completes jobs in random order); `precompile_overlaps_calls` (fake pool that records concurrent in-flight maximum; assert at least 4 on 20 files); `precompile_failure_isolated` (one failing file does not drop the others). **Acceptance:** cold `oxide build` of a fixture with 200 Svelte components at least 4x faster on an 8-core machine; warm build unchanged (the CAS short-circuits).

### V2-B08 Nonce splice without whole-document copies (S, depends on A01)

**Evidence.** `html.replace(&nonce, NONCE_SENTINEL)` copies the document once per prerendered page (`cli/src/lib.rs` about 3449, 3527, 3611) and in the live L0 fill (about 5306).

**Phase 1 (prerender, zero copies).** The prerender nonce is `ChunkBundler::content_hash(&[build_id])` (about `lib.rs:3286`), and prerendered pages are never served with that nonce (the sentinel is substituted at serve time). So render with `nonce = l0_cache::NONCE_SENTINEL` directly and delete the three `.replace` calls. Test: a prerendered file contains `NONCE_SENTINEL` and does not contain the old hash. Verify the sentinel is attribute-safe (`escape_attr` leaves it unchanged; it is used in `<script nonce='...'>`).

**Phase 2 (streaming splice).** A chunk-safe splicer for the live path, so neither the L0 fill nor the response path needs a full-document copy:

```rust
// crates/render/src/splice.rs  (add memchr to [workspace.dependencies] if absent; it is already in Cargo.lock)
pub struct NonceSplicer { from: Vec<u8>, to: Vec<u8>, finder: memchr::memmem::Finder<'static>, carry: Vec<u8> }

impl NonceSplicer {
    pub fn new(from: &str, to: &str) -> Self {
        let f = from.as_bytes().to_vec();
        Self { finder: memchr::memmem::Finder::new(&f).into_owned(), from: f, to: to.as_bytes().to_vec(), carry: Vec::new() }
    }
    /// Append the spliced form of `chunk` to `out`. Holds back the last from.len()-1 bytes so a
    /// match split across two chunks is still found.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        self.carry.extend_from_slice(chunk);
        let n = self.from.len();
        let mut i = 0;
        while let Some(p) = self.finder.find(&self.carry[i..]) {
            out.extend_from_slice(&self.carry[i..i + p]);
            out.extend_from_slice(&self.to);
            i += p + n;
        }
        let keep_from = self.carry.len().saturating_sub(n.saturating_sub(1)).max(i);
        out.extend_from_slice(&self.carry[i..keep_from]);
        let tail = self.carry[keep_from..].to_vec();
        self.carry = tail;
    }
    pub fn finish(&mut self, out: &mut Vec<u8>) { out.extend_from_slice(&self.carry); self.carry.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut s = NonceSplicer::new("NNNN", "abcdefgh"); let mut out = Vec::new();
        for c in chunks { s.push(c, &mut out); } s.finish(&mut out); out
    }
    #[test] fn splits_at_every_position() {
        let doc = b"<script nonce='NNNN'>x</script><b nonce='NNNN'>";
        let want = String::from_utf8_lossy(doc).replace("NNNN", "abcdefgh");
        for cut in 0..doc.len() { assert_eq!(String::from_utf8(run(&[&doc[..cut], &doc[cut..]])).unwrap(), want, "cut {cut}"); }
    }
    #[test] fn no_match_passthrough() { assert_eq!(run(&[b"hello ", b"world"]), b"hello world"); }
    #[test] fn byte_at_a_time() { let doc = b"aNNNNbNNNNc"; let parts: Vec<&[u8]> = doc.chunks(1).collect(); assert_eq!(run(&parts), b"aabcdefghbabcdefghc"); }
}
```

Integrate in the render stream tee where the L0 fill currently collects the full body: apply `NonceSplicer::new(&nonce, SENTINEL)` to the accumulating copy only, leaving the bytes sent to the client untouched; on a cache hit apply `NonceSplicer::new(SENTINEL, &nonce)` while writing the response. **Acceptance:** peak transient allocation for the L0 fill of a 200 KB page is under 1.1x the page size (dhat `max_bytes`); the prerender of the 5,000-page synthetic fixture no longer shows the 1 GB transient allocation.

### V2-B09 Benchmark baseline and harness (M, land first)

**Goal.** Numbers before changes, a repeatable harness, and a gate.

**Step 1: harness plumbing.** Add to root `[workspace.dependencies]`: `criterion` (default features off, `cargo_bench_support`) and `dhat`; pin versions from `cargo add --dry-run`. Add `[dev-dependencies]` plus `[[bench]] name = "..." harness = false` entries per crate below.

**Step 2: benches (create these files).**

| File | Cases |
| --- | --- |
| `crates/router/benches/match.rs` | `match/{10,100,1000,5000}_routes/{static,param,catchall,miss}`; build time of the tree and of `RouteContextTable` |
| `crates/cache/benches/keys.rs` | key construction (old `canonical()` and new `key_of`), tag insert with 10k keys, disk `set` with 5 tags |
| `crates/dispatch/benches/context.rs` | `baseline_scan` (a copy of the old per-request scans over N entries) against `RouteContextTable::get`; `run_chain` with 0, 1, 3 middleware (fake) |
| `crates/server/benches/metrics.rs` | `observe` single thread and 8 threads, registered vs unregistered label |
| `crates/render/benches/splice.rs` | `NonceSplicer` on 20 KB, 200 KB, 2 MB bodies against `str::replace` |

Synthetic route generator used by the router and dispatch benches (put in `crates/router/src/testing.rs` behind `#[cfg(any(test, feature = "bench-util"))]`):

```rust
pub fn synthetic_entries(n: usize) -> Vec<RouteEntry> {
    (0..n).map(|i| {
        let segs: Vec<String> = if i % 10 == 0 { vec![format!("s{}", i % 50), "[id]".into()] } else { vec![format!("s{}", i % 50), format!("p{i}")] };
        RouteEntry { id: RouteId(format!("routes/{}/page", segs.join("/"))), kind: RouteKind::Page, segments: segs,
            file: std::path::PathBuf::from("page.tsx"), render_mode: None, tags: vec![], has_server_module: false, needs_parent: false, slot: None }
    }).collect()
}
```

**Step 3: allocation budget test.** One integration test binary per budget (dhat allows one profiler per process): `crates/router/tests/alloc_match.rs` with `#[global_allocator] static A: dhat::Alloc = dhat::Alloc;`, build a 1,000-route tree, take `dhat::HeapStats::get()` before and after 1,000 `match_ref("/s7/p7")` calls, `dhat::assert_eq!(delta_blocks, 0)`. A second one in `oxide-dispatch` measures blocks per fake L0-hit request against the budget of 12.

**Step 4: summaries and the gate.**

```python
# benchmarks/criterion_summary.py <out.json> : median ns per benchmark from target/criterion
import json, glob, os, sys
out = {}
for p in glob.glob('target/criterion/**/new/estimates.json', recursive=True):
    name = os.path.relpath(os.path.dirname(os.path.dirname(p)), 'target/criterion')
    out[name] = json.load(open(p))['median']['point_estimate']
json.dump(out, open(sys.argv[1], 'w'), indent=1, sort_keys=True)
print(len(out), 'benchmarks')
```

```python
# benchmarks/criterion_compare.py <baseline.json> <new.json> [max_regress_percent=5]
import json, sys
b, n = json.load(open(sys.argv[1])), json.load(open(sys.argv[2]))
limit = float(sys.argv[3]) if len(sys.argv) > 3 else 5.0
bad = 0
for k in sorted(b):
    if k in n and b[k] > 0:
        pct = (n[k] - b[k]) / b[k] * 100
        flag = 'REGRESSION' if pct > limit else ''
        bad += 1 if flag else 0
        print('%-60s %12.1f -> %12.1f  %+7.1f%% %s' % (k, b[k], n[k], pct, flag))
sys.exit(1 if bad else 0)
```

Procedure: on the commit **before any B WP merges**, run `cargo bench --workspace`, then `python3 benchmarks/criterion_summary.py benchmarks/baseline-v01.json` and commit that file. Each B WP's PR runs the summary again and `criterion_compare.py` (improvements show as negative percentages; only regressions fail). Extend `benchmarks/gate.sh` to call it in CI after the existing table check.

**Step 5: end-to-end load.** `benchmarks/load.sh` builds the smoke app, starts `oxide-cli start` on a free port (background PID, watchdog kill), and runs `oha -z 10s -c 64 --no-tui --output-format json <url>` (fall back to `wrk` if `oha` is absent) against three URLs: an L0 hit, a dynamic SSR route and a hashed static asset; it prints `route rps p50_ms p99_ms rss_mb`. Append the three rows to the `benchmarks/report.md` table with the commit hash.

**Step 6: result table in `WORKLOG.md`** (fill per WP):

| Metric | Baseline | Target | After |
| --- | --- | --- | --- |
| `match/1000_routes/static` ns | recorded | 10x faster |  |
| allocations per static match | recorded | 0 |  |
| allocations per L0-hit request | recorded | at most 12 |  |
| `tag_insert/10k` | recorded | O(1) amortized |  |
| `disk_set_5_tags` | recorded | 3x faster |  |
| rps, L0 hit / dynamic SSR / static | recorded | report only |  |

**Acceptance.** `benchmarks/baseline-v01.json` committed before B01; `cargo bench -p oxide-router` runs in under 3 minutes; the CI gate fails a deliberately slowed matcher (prove it once, then revert).

## V2.7 Wave C: compilers and toolchain

The three research reports agree on direction (native Rust compilers behind a Node fallback) but their crate APIs, versions and maturity claims are **unverified by this spec**. Every C-series WP therefore starts with a written spike. The trait boundary, the config switch and the CAS keying are the stable part and ship first (C01).

### V2-C01 `NativeCompiler` trait, config switch, selection, CAS keys (M, no dependencies)

**Goal.** Let any framework compile in-process when a native compiler is compiled in and healthy, with the Node pool as the fallback, and never reuse an artifact across implementations.

**Evidence.** `precompile_entries` sends every Solid `.tsx`, `.vue` and `.svelte` to the pool (`compile_solid`, `compile_vue`, `compile_svelte` in `cli/src/precompile.rs`, worker kinds `tooling:solid-compile`, `tooling:vue-compile`, `tooling:svelte-compile`). `compiler/src/framework.rs` has `requires_tooling_pool()` for Vue, Svelte and Solid. `PrecompileKey` already has a `tool_version` field, so the CAS can distinguish implementations without a schema change.

**Files.** Create `crates/compiler/src/native/{mod.rs,solid.rs,vue.rs,svelte.rs,react_compiler.rs}`; modify `crates/compiler/Cargo.toml`, `crates/compiler/src/lib.rs` (`pub mod native;`), `crates/config/src/lib.rs` and `schema.rs`, `crates/cli/src/precompile.rs`, `crates/cli/Cargo.toml`, `crates/cli/src/commands/inspect.rs` (doctor).

```rust
// crates/compiler/src/native/mod.rs
use crate::{Result, err};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Generate { Dom, Ssr }

pub struct CompileRequest<'a> { pub path: &'a Path, pub source: &'a str, pub generate: Generate, pub dev: bool }

#[derive(Default, Debug)]
pub struct CompileOutput { pub js: String, pub css: Option<String>, pub warnings: Vec<String> }

/// String in, string out. No AST type may cross this boundary (each upstream pins its own oxc).
pub trait NativeCompiler: Send + Sync {
    fn framework(&self) -> &'static str;           // react | vue | svelte | solid
    fn id(&self) -> &'static str;                  // e.g. solid-native
    fn version(&self) -> String;                   // upstream version or vendored rev; part of the CAS key
    fn handles(&self, ext: &str) -> bool;
    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput>;   // synchronous, CPU bound
    fn health_fixture(&self) -> (&'static str, &'static str);                // (file name, tiny valid source)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Backend { #[default] Auto, Native, Node }

pub struct NativeRegistry { items: Vec<(Box<dyn NativeCompiler>, OnceLock<bool>)> }

impl NativeRegistry {
    pub fn from_enabled_features() -> Self {
        #[allow(unused_mut)]
        let mut items: Vec<(Box<dyn NativeCompiler>, OnceLock<bool>)> = Vec::new();
        #[cfg(feature = "native-solid")]  items.push((Box::new(super::native::solid::SolidNative::new()), OnceLock::new()));
        #[cfg(feature = "native-vue")]    items.push((Box::new(super::native::vue::VueNative::new()), OnceLock::new()));
        #[cfg(feature = "native-svelte")] items.push((Box::new(super::native::svelte::SvelteNative::new()), OnceLock::new()));
        Self { items }
    }

    fn healthy(&self, i: usize) -> bool {
        let (c, cell) = &self.items[i];
        *cell.get_or_init(|| {
            let (name, src) = c.health_fixture();
            let req = CompileRequest { path: Path::new(name), source: src, generate: Generate::Dom, dev: false };
            matches!(c.compile(&req), Ok(o) if !o.js.is_empty())
        })
    }

    /// Ok(None): use the Node pool. Ok(Some): compile in-process.
    pub fn pick(&self, framework: &str, ext: &str, backend: Backend) -> Result<Option<&dyn NativeCompiler>> {
        let found = self.items.iter().position(|(c, _)| c.framework() == framework && c.handles(ext));
        match (backend, found) {
            (Backend::Node, _) => Ok(None),
            (Backend::Auto, Some(i)) => Ok(if self.healthy(i) { Some(self.items[i].0.as_ref()) } else { None }),
            (Backend::Auto, None) => Ok(None),
            (Backend::Native, Some(i)) if self.healthy(i) => Ok(Some(self.items[i].0.as_ref())),
            (Backend::Native, _) => Err(crate::native_err(framework)),
        }
    }

    pub fn report(&self) -> Vec<(String, String, bool)> {      // for oxide doctor
        (0..self.items.len()).map(|i| (self.items[i].0.id().to_string(), self.items[i].0.version(), self.healthy(i))).collect()
    }
}
```

In `crates/compiler/src/lib.rs` add `pub(crate) fn native_err(fw: &str) -> OxideError` returning code `CMP-NAT-001`, message `framework.compilerBackend is 'native' but no healthy native compiler is available for {fw}`, fix `build oxide-cli with --features native-{fw} or set framework.compilerBackend to 'auto' or 'node'`.

**Cargo.**

```toml
# crates/compiler/Cargo.toml
[features]
default = []
native-solid = ["dep:solidjs-compiler"]
native-vue = ["dep:verter_core"]          # or the vize crates; decided by the C03 spike
native-svelte = ["dep:svelte-compiler"]
native-react-compiler = ["dep:oxc_react_compiler"]
[dependencies]
solidjs-compiler = { workspace = true, optional = true }
verter_core = { workspace = true, optional = true }
svelte-compiler = { workspace = true, optional = true }
oxc_react_compiler = { workspace = true, optional = true }
```

The dependency lines under `[workspace.dependencies]` are added **by the corresponding spike WP**, with the exact version it validated. `crates/cli/Cargo.toml` forwards each feature (`native-solid = ["oxide-compiler/native-solid"]`, and so on). Default dev builds omit them (fast compile); the release recipe in `justfile` enables all four.

**Config.** Add `compiler_backend: Option<String>` (`#[serde(rename = "compilerBackend")]`) to `FrameworkConfig` (`config/src/lib.rs:180`; the existing `compiler: Option<bool>` is the React Compiler flag, leave it). Schema: `"compilerBackend": { "enum": ["auto", "native", "node"] }` in `schema.rs`; env override `OXIDE_COMPILER_BACKEND`; default `auto`. Tests: config layering picks the env value over the file; invalid value gives a typo diagnostic with the three choices.

**Integration in `precompile.rs`** (replaces the `match kind` inside B07's `compile_one`):

```rust
async fn compile_one(cx: &CompileCx, kind: &str, path: &Path, source: &str, generate: &str) -> anyhow::Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if let Some(c) = cx.native.pick(kind, ext, cx.backend).map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))? {
        let (path, source) = (path.to_path_buf(), source.to_string());
        let gen_ = if generate == "ssr" { Generate::Ssr } else { Generate::Dom };
        let c: &'static dyn NativeCompiler = unsafe_free_static(c);       // see note below
        return tokio::task::spawn_blocking(move || {
            c.compile(&CompileRequest { path: &path, source: &source, generate: gen_, dev: false }).map(|o| o.js)
        }).await?.map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message));
    }
    match kind { "solid" => compile_solid(cx.pool, path, source, generate == "ssr").await, "vue" => compile_vue(cx.pool, path, source).await,
                 "svelte" => compile_svelte(cx.pool, path, source, generate).await, _ => anyhow::bail!("no compiler for {kind}") }
}
```

The `'static` note: hold the registry in an `Arc<NativeRegistry>` inside `CompileCx` and clone the `Arc` into the blocking closure (`let reg = Arc::clone(&cx.native)` then call `reg.pick(..)` inside); do not use `unsafe`. The CAS key's `tool_version` becomes `format!("native:{}@{}", c.id(), c.version())` for native and the existing pool version string otherwise, so switching `compilerBackend` never reuses a stale artifact (D40). Add `framework.compilerBackend` to the build fingerprint so the whole-build skip also invalidates.

**Doctor.** `oxide doctor` prints one line per registry entry: `native compiler solid-native 0.0.0@<rev> ok` or `... unhealthy` and warns when `compilerBackend` is `native` but an entry is missing.

**Equivalence harness (used by C02 to C04).** `scripts/compiler-equiv.sh <framework>`: build `examples/smoke-app-<fw>` twice (`OXIDE_COMPILER_BACKEND=node` then `native`), run `oxide-cli render /` and `oxide-cli render /blog/hello` (where present) for both, normalize (strip `data-v-*`, `svelte-*`, `s-*` class hash suffixes and `id="..."` counters with a small regex list kept in the script) and `diff`; then run the Playwright island click spec (E10) against both builds. A native compiler passes only when HTML is equal after normalization **and** the island test passes.

**Tests.** `registry_pick_node_always_none`, `registry_pick_native_missing_is_error` (no feature enabled: `CMP-NAT-001`), `registry_auto_falls_back_when_unhealthy` (a fake compiler whose fixture fails), `health_checked_once` (fake counts calls). Use a `FakeNative` implementing the trait under `#[cfg(test)]`.

### V2-C02 Solid native compiler (M, depends on C01)

**Spike first (timebox 1 day, record in `docs/decisions/C02-solid-native.md`).**

1. Vendor: `git clone --depth 1 https://github.com/solidjs/solid /tmp/solid && mkdir -p vendor && cp -r /tmp/solid/packages/compiler vendor/solidjs-compiler && git -C /tmp/solid rev-parse HEAD > vendor/solidjs-compiler/UPSTREAM_REV`. The upstream manifest probably inherits `version.workspace = true` and similar from the Solid monorepo; rewrite those fields to literals. Add `exclude = ["vendor/*"]` to the root `[workspace]` so Cargo does not try to adopt it.
2. Read `vendor/solidjs-compiler/src/lib.rs` for the public Rust entry point (report claims `crate-type = ["cdylib", "rlib"]`, features `node` and `tsrx`, and `oxc_* = 0.144`). Add `solidjs-compiler = { path = "vendor/solidjs-compiler", default-features = false }` to `[workspace.dependencies]`.
3. Write `#[ignore]` test `solid_spike` in `crates/compiler/tests/solid_native.rs` that compiles `fixtures/native/solid/Counter.tsx` (a signal, an event handler, a `<Show>`) for both DOM and SSR output and prints the code. Compare the output shape with the Node worker's output for the same file (`oxide-worker.mjs`, handler for `tooling:solid-compile`, note which options it passes: hydratable, generate, moduleName).
4. Measure: compile time for the fixture, `cargo tree -d | grep -c oxc` (duplicate oxc copies), release binary size delta.
5. **Kill criteria:** output cannot be made equivalent to the worker's (options not exposed); more than 90 s of added release compile time; or the crate fails to build on Windows. On kill, keep Node for Solid and record why.

**Build.** `SolidNative` in `native/solid.rs` maps `Generate::Dom|Ssr` and the options the worker passes onto the crate's entry point and returns the JS string. `framework() = "solid"`, `handles(ext)` true for `tsx` and `jsx`, `version()` reads `UPSTREAM_REV` via `include_str!("../../../../vendor/solidjs-compiler/UPSTREAM_REV")`. Gate: `scripts/compiler-equiv.sh solid` passes. **Acceptance:** cold build of the Solid fixture with 200 components at least 3x faster than the Node pool (same machine, empty CAS); `framework.compilerBackend: 'node'` still works and produces a different CAS key.

### V2-C03 Vue native compiler (M, depends on C01)

**Spike (2 days).** Candidates from the report: `verter_core` (pure Rust, `generate(input, &options, &allocator)`, macros `defineProps` and friends) and the `vize_atelier_*` family (reported benchmarks 4.3x single-threaded). Create `crates/compiler/tests/vue_native_spike.rs` (`#[ignore]`) that compiles three fixtures with each candidate: (a) `<script setup lang="ts">` with `defineProps` and a template with `v-for` and `v-model`, (b) scoped `<style>`, (c) a component using slots. For each record: API shape, TS handling (does it lower `lang=ts` itself, or must Oxc do it), scoped-style output and scope id stability across runs, output module imports (`vue` runtime names must match what `@vue/server-renderer` and the client adapter expect), and compile time. **Mirror the worker:** open `tooling:vue-compile` in `oxide-worker.mjs` and replicate every post-processing step it does (style injection, `__file` and HMR id, `export default` shape) so the returned `js` string has the same contract. Pick one candidate; document the other as rejected with the reason.

**Kill criteria:** hydration mismatch in the Vue smoke app (equivalence harness fails); scoped style ids differ between SSR and client builds; any macro unsupported that the Atlas app uses. Beta status (`0.0.1-beta.1`) means the feature stays opt-in (`auto` still falls back if `health_fixture` fails), and the Node path stays for at least one release.

### V2-C04 Svelte native compiler (M, depends on C01)

**Spike (2 days).** `svelte-compiler = 0.1.4` exposes `compile`, `compile_module`, `parse`, `print`, `preprocess`, `migrate`. Check: Svelte 5 runes (`$state`, `$derived`, `$props`), `generate: 'client' | 'server'` mapping to the worker's `dom` and `ssr`, CSS handling (`css: 'injected'` versus external), HMR option, and compile warnings text. Mirror the options that `tooling:svelte-compile` passes (read the handler). Because the Svelte adapter's `compile_svelte(.., generate)` receives `dom` or `ssr` strings, map them in `SvelteNative`. Same kill criteria as C03 plus: any Svelte 5 rune used by the smoke app or Atlas that the crate rejects.

### V2-C05 React Compiler, optional (S, depends on C01)

The React path is already native (`oxc` transformer, `ReactInline`). The only addition is the optional React Compiler port. Spike: does `oxc_react_compiler` (report: v0.137.0) build next to workspace `oxc 0.153.0`, and what is its entry point. Wire it behind `native-react-compiler` and the existing `framework.compiler: true` flag, applied to `.tsx` after JSX lowering is **not** correct (the compiler needs the pre-lowering AST); apply it first, string in and string out, then run the normal transform. Gate: React smoke app renders identically (hooks memoized, no hydration errors) and `oxide analyze` shows route JS not larger than before. Skip the WP entirely if the spike shows a hard oxc skew that cannot link.

### V2-C06 Source-map composition in the plugin chain (S, no dependencies)

**Evidence.** The JS and WASM plugin `transform` hooks return code only (`build_js_plugin_hook`, `build_transform_hook`), so a plugin that rewrites code breaks the chain from final output back to source. The compiler crate already depends on `oxc_sourcemap` 9.0.0.

**Design.** Extend the hook result with an optional map and compose maps in order. Algorithm (pure, testable without any engine): given `prev` (maps intermediate positions to original) and `next` (maps final positions to intermediate), for every token of `next`, take its (intermediate line, column) and find the greatest token in `prev` at or before that position on the same line; emit a token with the final position and `prev`'s original source, line, column and name. Drop tokens with no match.

```rust
// crates/bundler/src/sourcemap_compose.rs
#[derive(Clone, Debug, PartialEq)]
pub struct Tok { pub gen_line: u32, pub gen_col: u32, pub src: u32, pub src_line: u32, pub src_col: u32 }

/// prev: tokens sorted by (gen_line, gen_col). next: tokens whose src_line/src_col index into prev's generated space.
pub fn compose(prev: &[Tok], next: &[Tok]) -> Vec<Tok> {
    let mut out = Vec::with_capacity(next.len());
    for n in next {
        // last prev token with (gen_line, gen_col) <= (n.src_line, n.src_col)
        let i = prev.partition_point(|p| (p.gen_line, p.gen_col) <= (n.src_line, n.src_col));
        if i == 0 { continue; }
        let p = &prev[i - 1];
        if p.gen_line != n.src_line { continue; }
        out.push(Tok { gen_line: n.gen_line, gen_col: n.gen_col, src: p.src, src_line: p.src_line, src_col: p.src_col });
    }
    out
}
```

Convert to and from `oxc_sourcemap::SourceMap` tokens with the crate's accessors (read `cargo doc -p oxc_sourcemap`); keep `compose` itself independent of the crate so its tests need no engine. Tests: identity map composes to itself; a one-line shift (prefix of 10 columns) composes correctly; a token with no predecessor on its line is dropped; three-stage composition equals two-stage-then-one. Wire: JS plugin protocol `plugin:call` result may now carry `map` (a V3 JSON string); in `build_transform_hook` compose with the current map for that module before passing to the bundler. **Acceptance:** a fixture plugin that prepends a banner line yields a final `.map` whose first original-code token still points at line 1 of the source file (assert by decoding the emitted `.map`).

### V2-C07 SVG optimizer (S, no dependencies)

Add a trait in `crates/assets/src/svg.rs`: `pub trait SvgOptimizer: Send + Sync { fn optimize(&self, svg: &str) -> Result<String>; }` with `IdentitySvg` (default) and `OxvgSvg` behind feature `svg-oxvg` (crate `oxvg_optimiser`, report version 0.0.5; verify with the same spike rule: a test optimizing three fixtures: one with `<use href="#a">`, one with inline `<style>`, one with `viewBox` only). Rules the wrapper enforces after optimizing: the `viewBox` attribute is preserved, ids referenced by `<use>`, `url(#...)` or `aria-labelledby` are not removed or renamed, output is well-formed XML (parse it back). Config `assets.svg: { optimize: true }`. Hook: SVG imports with `?url`, `?raw`, `?inline` and SVG-as-component; results CAS-keyed by content hash plus optimizer id and version. **Acceptance:** the fixture set shrinks at least 15% in total and every `<use>` target survives.

### V2-C08 Glyph-usage font subsetting (M, no dependencies)

**Evidence.** `emit_fonts` subsets by a static list. The deps report proposes `fontcull` (built on fontations/klippa, WOFF2).

**Design.** Glyph-usage subsetting is only sound when all text is known, so scope it to **fully static output**: `fonts.subset: 'used'` is allowed only when every route is `static` (the build refuses otherwise, `AST-FNT-001`, fix: use `unicode-range` or `latin`). Steps: (1) after prerender, collect text per page with `lol_html` (`text!` handler over `body`) into one `BTreeSet<char>` per font family declared in `fonts` config (single set per family is the honest simplification; per-element `font-family` resolution is out of scope); (2) include the always-needed set (ASCII digits, space, common punctuation) and the content-collection field text; (3) `trait FontSubsetter { fn subset(&self, font: &[u8], chars: &BTreeSet<char>) -> Result<Vec<u8>>; }` with the existing backend and a `FontcullSubsetter` behind `fonts-fontcull`; (4) emit WOFF2, preload and metric-override CSS exactly as today. Test: subset a fixture font with `{"a","b"}` and check via `skrifa` that the cmap contains those two and not `c`; the build with `subset: 'used'` on a dynamic route fails with `AST-FNT-001`. **Acceptance:** a static docs fixture's font payload drops at least 60% versus the `latin` subset.

### V2-C09 Parallel, resumable image job model (S, no dependencies)

**Evidence.** The image endpoint transforms per request; the report suggests copying `rimage`'s job model rather than swapping codecs.

**Design.** In `crates/assets/src/images.rs` add `ImageJobs`: (a) a bounded `Semaphore` sized `available_parallelism() - 1` so image CPU work cannot starve render; (b) **single-flight** per output key (`DashMap<KeyStr, Shared<BoxFuture<Result<Arc<Vec<u8>>>>>>`) so N concurrent requests for the same variant do one transform; (c) CAS lookup before work, CAS store after (the existing image cache key already hashes inputs); (d) encoders run in `spawn_blocking`, and AVIF encoding gets its own smaller semaphore (speed setting from config `images.avifSpeed`, default 6). Tests: 50 concurrent identical requests produce one encode (count via a fake backend); a failed encode is not cached and the next request retries; the semaphore bounds concurrency (fake backend records the maximum in flight). **Acceptance:** `oha -c 64` against one image variant on a cold cache shows exactly one encode in the logs.

### V2-C10 MDX native evaluation (spike, M)

The deps report names `snapfire_fsr_mdx` (compiles `.mdx` to a TSX module, built on `markdown-rs` and `swc`) and `@gentleduck/md`. Current content handling is `crates/assets/src/content.rs` and `content_collection.rs` (comrak, typed collections). The decisive question is **adapter neutrality**: MDX compiles to JSX, which only React and Solid consume directly. Spike deliverable (`docs/decisions/C10-mdx.md`): can the candidate emit adapter-specific output, or must Oxide keep its own Markdown-to-component pipeline per adapter. Adopt only if (a) it works for all four adapters, (b) a second JS parser (`swc`) in the binary is accepted by the size budget, and (c) remark and rehype plugin absence is documented. Otherwise keep comrak and close the WP.

### V2-C11 Lint, format and typecheck backends (S, no dependencies)

`oxide check` already runs `tsgo` and `oxlint` when present (P1-17). Extend `cmd_check` (`commands/inspect.rs`): config `check.typecheck: 'tsgo' | 'oxlint' | 'tsrs' | 'tsc' | 'off'` (default `tsgo`); `oxlint` mode runs `oxlint --type-aware --type-check` (it orchestrates the Go-based `tsgolint`; `doctor` says so); `tsrs` is opt-in and labeled experimental. Add `oxide fmt [--check]` that shells out to `oxfmt`, falling back to `biome format` when absent, and a loud `SKIPPED` line otherwise (same convention as P1-17; `OXIDE_CHECK_STRICT=1` turns a missing tool into a failure). Resolution helper:

```rust
fn resolve_tool(name: &str, root: &std::path::Path) -> Option<std::path::PathBuf> {
    let local = root.join("node_modules/.bin").join(name);
    if local.is_file() { return Some(local); }
    which(name)          // existing helper in commands/inspect.rs
}
```

Tests: fake executables written to a temp `node_modules/.bin` that exit 0 or 1 drive the three outcomes (pass, fail, missing-is-SKIPPED, missing-is-failure under strict). Doctor lists each tool and version.

## V2.8 Wave F: the twenty v0.2 features

Design source: `docs/oxide-v0.2-feature-designs.md`. This section turns each design into an implementable WP. Wave 0 of that document (router boot, layouts, defer values, real hydration, `route.ts`, middleware, loading, streaming) is **already done** (V2.1.1), so F01, F03 and F05 are unblocked. Every F WP ends with three obligations (V2.0 item 12): conformance assertion, capability-matrix row (R02), config schema key.

### V2-F01 Mixed-framework islands (M, depends on A04)

**Goal.** React, Vue, Svelte and Solid islands on one page; each hydrated by its own adapter and rendered on the server by its own renderer.

**Evidence.** The boot entry (`generate_island_boot_entry`, now `boot_entry.rs`) static-imports every island and one `clientAdapter` for the project framework; `islands.ts` calls that single adapter's `hydrateIsland`. The island markup (`<ox-island data-ox-island data-ox-chunk data-ox-strategy data-ox-props>`) is byte-identical across adapters, and props travel as ODF base64, so the protocol is already neutral.

**Design.**

1. **Config.** `islands` values accept a string (today) or an object: `islands: { Counter: 'src/islands/counter.tsx', Price: { src: 'src/islands/Price.vue' }, Ticker: { src: 'src/islands/ticker.tsx', framework: 'solid' } }`. Inference order for an island's framework: explicit `framework`; file extension (`.vue` is vue, `.svelte` is svelte); a first-line pragma `// @oxide-framework solid` for `.tsx`; else the project `framework`. Put the function in `config_load.rs`:

```rust
pub(crate) fn island_framework(src: &str, explicit: Option<&str>, project: &str, first_line: &str) -> String {
    if let Some(f) = explicit { return f.to_string(); }
    match std::path::Path::new(src).extension().and_then(|e| e.to_str()) {
        Some("vue") => return "vue".into(),
        Some("svelte") => return "svelte".into(),
        _ => {}
    }
    if let Some(rest) = first_line.trim().strip_prefix("// @oxide-framework ") { return rest.trim().to_string(); }
    project.to_string()
}
```

2. **Generated boot entry** (replace the static imports):

```js
// Generated by oxide-cli. Do not edit.
import { installBootRuntime, bootWithRouter } from 'oxide/client/boot';
const adapters = {            // only frameworks present in this project; each is a lazy chunk
  react: () => import('@oxide/react/client'),
  vue: () => import('@oxide/vue/client'),
};
const islands = {
  Counter: { fw: 'react', load: () => import('/abs/src/islands/counter.tsx') },
  Price:   { fw: 'vue',   load: () => import('/abs/src/islands/Price.vue') },
};
installBootRuntime({ adapters, islands });
```

Also write `.oxide/islands.json` (`{ Name: { chunk, framework } }`) at build for tooling (`oxide why`, F17). Rolldown `advancedChunks` groups `react`, `react-dom`, `vue`, `svelte`, `solid-js` into per-framework vendor chunks so each loads once, on the first island of that framework. 3. **Client.** In `packages/oxide/src/client/islands.ts` the hydrate step becomes:

```ts
async function hydrateOne(host: Element, reg: Registry) {
  const name = host.getAttribute('data-ox-island')!
  const entry = reg.islands[name]
  if (!entry) return markError(host, 'unknown island ' + name)
  const [mod, adapter] = await Promise.all([entry.load(), reg.adapters[entry.fw]().then(m => m.clientAdapter)])
  const props = decodeProps(host.getAttribute('data-ox-props'))
  const dispose = await adapter.hydrateIsland!(host, mod, props)
  host.setAttribute('data-ox-hydrated', 'done')
  return dispose
}
```

`installBootRuntime` keeps its old one-adapter signature working (`adapter` shorthand expands to `{ adapters: { [fw]: () => ... } }`), so existing apps and tests do not break. 4. **Server rendering of foreign islands.** The page's own adapter renders islands of its framework as today. For a foreign island the page adapter's `Island` component (`packages/*/src/island.ts`) emits a self-contained placeholder `<ox-island data-ox-island='Price' data-ox-fw='vue' data-ox-pending='1' data-ox-props='...'></ox-island>`. A core stream transform in `packages/oxide/src/server/foreign-islands.ts` wraps the adapter's render stream, finds placeholders, renders each with the island's own adapter and splices the HTML in before the chunk is flushed:

```ts
export function resolveForeign(src: ReadableStream<Uint8Array>, render: (fw: string, name: string, propsB64: string) => Promise<string>) {
  const dec = new TextDecoder(), enc = new TextEncoder()
  let carry = ''
  const RE = /<ox-island([^>]*)data-ox-pending='1'([^>]*)><\/ox-island>/g
  return src.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
    async transform(chunk, ctl) {
      let s = carry + dec.decode(chunk, { stream: true })
      // hold back an unfinished '<ox-island' tag so a placeholder split across chunks is still seen
      const cut = s.lastIndexOf('<ox-island')
      const unfinished = cut >= 0 && s.indexOf('</ox-island>', cut) < 0
      carry = unfinished ? s.slice(cut) : ''
      if (unfinished) s = s.slice(0, cut)
      const parts: string[] = []; let last = 0; let m: RegExpExecArray | null
      while ((m = RE.exec(s))) {
        parts.push(s.slice(last, m.index))
        const attrs = m[1] + m[2]
        const get = (k: string) => new RegExp(k + "='([^']*)'").exec(attrs)?.[1] ?? ''
        const html = await render(get('data-ox-fw'), get('data-ox-island'), get('data-ox-props'))
        parts.push('<ox-island' + attrs + '>' + html + '</ox-island>')   // pending flag removed
        last = m.index + m[0].length
      }
      parts.push(s.slice(last))
      ctl.enqueue(enc.encode(parts.join('')))
    },
    flush(ctl) { if (carry) ctl.enqueue(enc.encode(carry)) },
  }))
}
```

`render` is provided by the worker host: every adapter in the project registers its island renderer in the isolate (`globalThis.__oxide_island_renderers[fw]`), added to the worker op table as `island:render` with parity on V8 (A08 pattern). Unit-test `resolveForeign` with a fake `render`, chunks split at every byte offset. 5. **Per-file compile settings.** `precompile_entries` today takes one `framework`; give it `file_framework: BTreeMap<PathBuf, String>` built from the islands config so a `.tsx` Solid island compiles with the Solid compiler while React pages keep Oxc JSX. Same for `jsx_import_source_for`: add per-file overrides in `bundle_env_options` (`react` versus `solid-js`).

**Tests.** Rust: `island_framework_inference` (table of 8 cases). TS: `boot.test.ts` mixed registry hydrates two fake adapters independently; `foreign-islands.test.ts` split-chunk property test. Fixture: `examples/mixed-islands` (React page, one island per framework, shared prop shape `{label, start}`). Conformance assertion `mixed islands`: all four hydrate and each increments only its own state. Playwright (E10): click each of four islands; each shows its own count.

**Acceptance.** One document with four vendor chunks loaded on demand (network log), zero console errors; `oxide analyze` lists per-framework vendor sizes; matrix row `mixedIslands` is Full on Binary, Node, Docker, Degraded on edge until E09.

**Risks.** Four runtimes on one page cost bytes (documented, strategy-gated). Hydration of React 19 next to Vue and Solid in one document is the open question; the fixture exists to find problems early. If one pair is incompatible, record it in the capability table instead of hiding it.

### V2-F02 Auto-islands inference (L, spike, depends on F01)

**One-week spike with kill criteria; ship only if it passes.** Extend `crates/compiler` with `scan_interactivity(source, framework) -> Interactivity { interactive: bool, reasons: Vec<Reason> }` (Oxc AST, direct detection only): React (`useState`, `useReducer`, `useEffect`, `useLayoutEffect`, `useRef` with `.current` writes, any `on*` JSX attribute), Vue (`ref`, `reactive`, `computed`, `watch`, `onMounted`, `emit`, `v-on` or `@event` in template via the SFC), Svelte (`$state`, `$:` statements, `on:` or `onclick` attributes, `bind:`), Solid (`createSignal`, `createEffect`, `createStore`, `on*`). A component is pure when it has no hits and imports no interactive component from the same project. Output `.oxide/island-manifest.json` `{ file: { interactive, reasons } }`; non-interactive route components are omitted from the hydration set (their SSR output is unchanged). Gated by `experiments.autoIslands: true`; explicit `islands` registration always wins; dev warns when a pure-classified component receives `on*` props. **Kill:** any false negative across the four examples, Atlas and the mixed fixture; or less than 15% route-JS saving on the examples. Write the decision to `docs/decisions/F02-auto-islands.md`. A wrong inference silently breaks interactivity, so the default is to defer to v0.3.

### V2-F03 Interaction replay (S, no dependencies)

**Goal.** Clicks, input and focus that land before an island hydrates are replayed after it does.

**Design.** New core file `packages/oxide/src/client/replay.ts` (about 1 KB minified), started from `boot.ts` before any island loads.

```ts
type Rec = { type: 'click' | 'input' | 'change' | 'focus'; island: Element; path: number[]; value?: string; checked?: boolean }
const MAX = 100
const log: Rec[] = []

function pathOf(root: Element, el: Element): number[] {
  const p: number[] = []
  for (let n: Element | null = el; n && n !== root; n = n.parentElement) p.unshift(Array.prototype.indexOf.call(n.parentElement!.children, n))
  return p
}
function resolve(root: Element, path: number[]): Element | null {
  let n: Element | null = root
  for (const i of path) { n = n?.children[i] ?? null }
  return n
}
function pendingIsland(t: EventTarget | null): Element | null {
  const host = (t as Element | null)?.closest?.('ox-island')
  return host && host.getAttribute('data-ox-hydrated') !== 'done' ? host : null
}

export function startReplay(doc: Document = document) {
  const cap = (type: Rec['type']) => (e: Event) => {
    const host = pendingIsland(e.target); if (!host) return
    const el = e.target as HTMLInputElement
    log.push({ type, island: host, path: pathOf(host, el), value: el.value, checked: el.checked })
    if (log.length > MAX) log.shift()
  }
  const types: Rec['type'][] = ['click', 'input', 'change', 'focus']
  const fns = types.map(t => [t, cap(t)] as const)
  fns.forEach(([t, f]) => doc.addEventListener(t === 'focus' ? 'focusin' : t, f, true))
  return () => fns.forEach(([t, f]) => doc.removeEventListener(t === 'focus' ? 'focusin' : t, f, true))
}

/** Call right after an island's hydrate resolves; replays that island's records in order. */
export function replayFor(host: Element) {
  for (let i = 0; i < log.length;) {
    const r = log[i]
    if (r.island !== host) { i++; continue }
    log.splice(i, 1)
    const el = resolve(host, r.path) as HTMLInputElement | null
    if (!el || !host.contains(el)) continue            // element replaced by hydration or a defer swap: drop
    if (r.type === 'click') el.click()
    else if (r.type === 'focus') el.focus()
    else { if (r.value !== undefined) el.value = r.value; el.dispatchEvent(new Event(r.type, { bubbles: true })) }
  }
}
```

Wire `replayFor(host)` in `islands.ts` immediately after `data-ox-hydrated` becomes `done`. Config `interactionReplay: false` disables it (the boot runtime then skips `startReplay`). **Invariants:** an event that reached a handler because hydration finished mid-flight must not replay twice: capture records only when the island was pending at capture time (checked by `pendingIsland`), which is exactly the window in which no handler could have run; events targeting `template[data-ox-b]` fallback content are dropped because the path no longer resolves.

**Tests** (`packages/oxide/test/replay.test.ts`, fake DOM): click before hydrate replays once; click after hydrate is not recorded; input value and event replay; capacity drop-oldest at 100; unresolvable path dropped; focus replay. **Acceptance:** Playwright blocks the island chunk request, clicks the SSR button, releases the request, and the handler count is exactly 1. Same for a text input.

### V2-F04 Cache-safe personalization (M, depends on A05)

**Goal.** A/B experiments evaluated natively become L0 variant keys, so experiments do not force `dynamic`.

**Files.** `crates/dispatch/src/experiments.rs`; config `experiments`; stage `stage_experiments` between prelude and L0 lookup; `l0_cache.rs` vary string.

```rust
// crates/dispatch/src/experiments.rs
#[derive(Clone, Debug, PartialEq)]
pub enum Sticky { Cookie(String), Header(String), IpHash }

#[derive(Clone, Debug)]
pub struct Experiment { pub id: String, pub traffic: u8, pub sticky: Sticky, pub variants: Vec<String>, pub routes: Vec<String> /* URL pattern prefixes; empty = all */ }

/// Deterministic assignment: None means not in the experiment.
pub fn assign(exp: &Experiment, sticky_value: &str) -> Option<usize> {
    if exp.variants.is_empty() || sticky_value.is_empty() { return None; }
    let mut h = blake3::Hasher::new();
    h.update(exp.id.as_bytes()); h.update(&[0]); h.update(sticky_value.as_bytes());
    let b = h.finalize();
    let n = u64::from_le_bytes(b.as_bytes()[..8].try_into().unwrap());
    if (n % 100) as u8 >= exp.traffic { return None; }
    Some(((n / 100) % exp.variants.len() as u64) as usize)
}

pub struct Resolved { pub assigned: std::collections::BTreeMap<String, String>, pub set_cookie: Option<String>, pub vary_suffix: String }

pub fn resolve(exps: &[Experiment], path: &str, cookie_header: Option<&str>, header: &dyn Fn(&str) -> Option<String>, ip: &str, mint: &dyn Fn() -> String) -> Resolved {
    let mut assigned = std::collections::BTreeMap::new();
    let mut set_cookie = None;
    for e in exps.iter().filter(|e| e.routes.is_empty() || e.routes.iter().any(|r| path.starts_with(r.as_str()))) {
        let sticky_val = match &e.sticky {
            Sticky::Cookie(name) => match cookie_value(cookie_header, name) {
                Some(v) => v,
                None => { let v = mint(); set_cookie = Some(format!("{name}={v}; Path=/; Max-Age=2592000; SameSite=Lax; HttpOnly")); v }
            },
            Sticky::Header(h) => header(h).unwrap_or_default(),
            Sticky::IpHash => ip.to_string(),
        };
        if let Some(i) = assign(e, &sticky_val) { assigned.insert(e.id.clone(), e.variants[i].clone()); }
    }
    let vary_suffix = assigned.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";");
    Resolved { assigned, set_cookie, vary_suffix }
}

fn cookie_value(h: Option<&str>, name: &str) -> Option<String> {
    h?.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
}
```

**Wiring.** (1) The vary string used for the L0 key gets `|exp:` + `vary_suffix` **only when non-empty**, so keys for routes outside any experiment stay byte-identical. (2) `Resolved.assigned` goes to `RequestLocals.experiments` (A05) so loaders and the render input see it. (3) `set_cookie` is added to the response headers **after** the L0 lookup and never stored in the cached entry (strip `set-cookie` from `make_entry` headers if not already). (4) Metrics: counter `oxide_experiment_assignment{experiment,variant}`; `oxide why <route>` prints the key breakdown including the experiment dimension. (5) Config schema: `experiments: [{ id, traffic, sticky: 'cookie'|'header'|'ip-hash', cookieName?, headerName?, variants, routes? }]`; validate `traffic` 0 to 100 and at least 2 variants (`DSP-110`).

**Tests.** `assignment_is_deterministic`; `traffic_zero_assigns_nobody`; `traffic_100_assigns_everyone`; `distribution_within_5_percent_over_10000_ids`; `cookie_minted_when_absent_and_stable_when_present`; `vary_suffix_empty_without_experiments` (key bytes equal the pre-F04 key); integration: two cookie identities in different buckets receive different cached HTML and each variant is a separate L0 hit. **Acceptance:** with no `experiments` configured, L0 keys and hit rate equal the pre-F04 baseline.

### V2-F16 "Why dynamic?" with quick fixes (S, no dependencies)

**Goal.** Say why a route is dynamic, where, and fix it with a verified codemod.

**Evidence.** `infer_render_mode` (`compiler/src/lib.rs:956`) already walks the AST and returns `InferredMode { mode, trigger_span, trigger }` for the **first** request-specific read only.

**Design.**

1. **Collect all reads.** Add `pub fn infer_reasons(source, path) -> Vec<DynamicReason>` beside it, reusing the same visitor but pushing every hit:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicReason { pub kind: ReasonKind, pub span: (usize, usize), pub line: u32, pub snippet: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonKind { CookieRead, HeaderRead, LocalsRead, SearchParamRead, UncachedFetch }
```

`kind` maps from the existing trigger strings (`cookies`, `request.headers`, `locals`, `searchParams`); `UncachedFetch` is a `fetch(` call not inside a `'use cache'` function. `infer_render_mode` stays as the cheap first-hit entry point (its tests must not change). 2. **Manifest.** Store reasons per route in the routes manifest JSON side-car (not the rkyv archive, which the hot path reads): `.oxide/routes.reasons.json`. 3. **Surfaces.** `oxide why <route>` prints each reason with a code frame (the diagnostics crate has `code_frame`); the inspector route table gets a `why` column; diagnostics code `OX-DYN`. 4. **Codemods** run in the tooling pool (TS transform): `CookieRead` in render code becomes a loader read returned as data; `HeaderRead` likewise through `setHeaders` or loader data; `SearchParamRead` becomes a declared `searchSchema` (E02) so the route is cacheable per param; `UncachedFetch` gets `'use cache'`. `oxide why <route> --fix` applies one codemod, re-runs `infer_reasons`, prints before and after mode, and refuses to continue if the mode did not improve (verification by the same engine that found the problem). Multi-reason routes fix one reason per run.

**Tests.** Four fixtures (`fixtures/why/{cookie,header,search,fetch}.server.ts`); each has expected reasons, then after `--fix` zero reasons and mode `static` or `isr`; `fix_is_idempotent` (second run changes nothing); `comment_and_string_mentions_are_not_reasons`. **Acceptance:** `oxide why <route>` on a dynamic fixture prints the line number and snippet; `--fix` flips the printed mode.

### V2-F10 Skew protection and atomic deploys (M, depends on A04)

**Goal.** After a deploy, tabs still on the old build keep working for a window, and rollback is instant.

**Evidence/dependencies.** A04 provides the build id, `GenerationSet` and the 409 contract. The worker pool already has route affinity (PERF-02); generations extend it.

**Design.**

1. **Two server generations.** `oxide start` loads `dist/` as generation G1. On `SIGHUP` or `oxide start --watch-dist`, it loads the new `dist/` as G2, calls `GenerationSet::promote`, and keeps G1's **server module and pool** alive for `deploy.skewWindow` (default `15m`). Implementation: `GenerationPools { current: Arc<dyn Pool>, previous: Option<(String, Arc<dyn Pool>, Instant)> }` in `crates/cli/src/serve/generations.rs`; `Pool` construction already takes the dist path.
2. **Routing by build.** Data-plane requests (`/__data`, `/_oxide/fn/*`, actions) with `x-oxide-build` equal to the previous id are dispatched to the previous pool (and previous L0 namespace, since L0 keys include the build id); everything else, including all HTML, goes to current. Unknown or expired id gives `409` with `x-oxide-build-stale: true` (A04 client reloads once).
3. **Expiry.** A background task checks every 30 s; at window end it drops the previous pool and unlinks nothing (client chunks are content-hashed and stay valid). Memory is bounded at two generations; when `oxide_pool_queue_wait_p99_us` exceeds the brownout threshold (F12) the previous generation is dropped early.
4. **Rollback.** Keep `.oxide/generations/<id>` as hard-linked copies (or symlinks on Windows fallback to copy) of the last N=3 builds, written at the end of `phase_emit`. `oxide start --generation previous` or `oxide rollback` repoints `dist` and sends `SIGHUP`. No rebuild.
5. **Adapters.** Vercel, Netlify and Cloudflare adapters pass the build id as an env var and document that skew is `Window` where the platform keeps old deployments reachable. Add `Skew` to the capability matrix (R02): Full on Binary and Docker; Window on Node; Degraded on serverless; Unsupported on Static (CDN content hashing only).

**Tests.** `GenerationSet` tests (A04); `previous_pool_serves_old_build_header` with two fake pools returning their build id; `expired_generation_returns_409`; `html_always_current`; `rollback_swaps_dist` against a temp dir. E2E (`tests/skew.sh`): start v1, open a page and capture its build id, rebuild with a changed server function, send `SIGHUP`, call the old server-fn id with the old header (succeeds, v1 output), fetch HTML (v2), advance the window with `--skew-window 2s` and call again (409).

**Spec impact:** Part I §12 gains "Build generations and skew" (V2.3 D28). **Acceptance:** the e2e script passes on Linux and macOS.

### V2-F05 Live loaders over SSE (M, depends on A04, A05)

**Goal.** A route marked live re-runs its loaders when a tag it depends on is revalidated, and pushes the new data to open tabs with no hand-written socket code.

**Evidence.** There is no SSE in the tree. The pieces that exist: tag revalidation (`TieredCache::invalidate_tag`, `/_oxide` revalidate endpoint), streaming responses (`OxideResponse::stream(status, BodyStream)`, never compressed because `compressible` requires `resp.stream.is_none()` at `server/src/lib.rs:3139`), the client's in-place data application for HMR (`applyHmrMessage` in `packages/oxide/src/client/hmr.ts`, message `ox:data`), and `LoaderRunner`.

**Design.**

1. **Opt-in.** `export const config = { live: true }` in `page.server.ts` (or `live: { debounceMs: 100 }`). The compiler reads it with `read_page_config` (router scan) into `RouteEntry.live` (additive field, rkyv archive version bump). `hydration_bootstrap` adds `live:true` to `window.__oxide` for such routes.
2. **Endpoint.** `GET /__oxide/live?route=<urlpattern id>&url=<path+query>&build=<id>`. Data plane (A04 build check applies; EventSource cannot set headers, so the build id travels in the query). Same-origin only (`Origin`/`Sec-Fetch-Site`). Response: `200`, `content-type: text/event-stream`, `cache-control: no-store`, `x-accel-buffering: no`, body is a stream.
3. **Events.**

```
event: data\nid: 7\ndata: <base64 ODF or ODP1 patch (F06)>\n\n     # new snapshot for the route's loader data
event: ping\ndata: 1\n\n                                      # every live.keepaliveSeconds (default 20)
event: stale\ndata: <new build id>\n\n                      # build changed; client reloads once (A04)
```

A fresh connection immediately receives a full `data` snapshot, so reconnect after a drop never needs replay. 4. **Hub.** `crates/server/src/live.rs`:

```rust
use bytes::Bytes;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak, atomic::{AtomicUsize, Ordering}};
use tokio::sync::watch;

pub struct Sub {
    pub route: String,
    pub url: String,
    pub headers: Vec<(String, String)>,         // cookie snapshot taken at connect; lives only as long as the connection
    pub tags: Mutex<HashSet<String>>,           // route config tags + tags recorded by the last run
    pub tx: watch::Sender<Option<Bytes>>,       // latest snapshot wins; slow clients skip intermediate states
    pub seq: AtomicUsize,
}

pub struct LiveHub { by_tag: dashmap::DashMap<String, Vec<Weak<Sub>>>, count: AtomicUsize, max: usize }

pub struct Guard { hub: Arc<LiveHub>, sub: Arc<Sub> }
impl Drop for Guard { fn drop(&mut self) { self.hub.remove(&self.sub); } }

impl LiveHub {
    pub fn new(max: usize) -> Arc<Self> { Arc::new(Self { by_tag: Default::default(), count: AtomicUsize::new(0), max }) }
    /// Err when over capacity: the endpoint answers 503 with retry-after.
    pub fn subscribe(self: &Arc<Self>, sub: Arc<Sub>) -> Result<Guard, ()> {
        if self.count.fetch_add(1, Ordering::AcqRel) >= self.max { self.count.fetch_sub(1, Ordering::AcqRel); return Err(()); }
        for t in sub.tags.lock().unwrap().iter() { self.by_tag.entry(t.clone()).or_default().push(Arc::downgrade(&sub)); }
        Ok(Guard { hub: Arc::clone(self), sub })
    }
    fn remove(&self, sub: &Arc<Sub>) {
        self.count.fetch_sub(1, Ordering::AcqRel);
        for t in sub.tags.lock().unwrap().iter() {
            if let Some(mut v) = self.by_tag.get_mut(t) { v.retain(|w| w.upgrade().map_or(false, |s| !Arc::ptr_eq(&s, sub))); }
        }
    }
    /// Subscribers that depend on `tag` (deduplicated). The caller schedules their reruns.
    pub fn affected(&self, tag: &str) -> Vec<Arc<Sub>> {
        let mut out: Vec<Arc<Sub>> = Vec::new();
        if let Some(v) = self.by_tag.get(tag) {
            for w in v.iter() { if let Some(s) = w.upgrade() { if !out.iter().any(|o| Arc::ptr_eq(o, &s)) { out.push(s); } } }
        }
        out
    }
}

/// SSE framing, kept pure so it is trivially testable.
pub fn sse_frame(event: &str, id: Option<usize>, data: &str) -> String {
    let mut s = format!("event: {event}\n");
    if let Some(i) = id { s.push_str(&format!("id: {i}\n")); }
    for line in data.split('\n') { s.push_str(&format!("data: {line}\n")); }
    s.push('\n');
    s
}
```

Tag matching supports a trailing wildcard (`comments:*`): store wildcard tags under their prefix and check `starts_with` in `affected` (add `by_prefix: DashMap<String, Vec<Weak<Sub>>>`). 5. **Rerun worker.** `LiveRunner` owns an `mpsc::Receiver<Arc<Sub>>`, debounces per subscriber by `debounceMs` (default 50), caps `maxRerunsPerSecond` per subscriber (default 20), calls the injected `rerun: Arc<dyn Fn(Arc<Sub>) -> BoxFuture<'static, Option<Bytes>> + Send + Sync>` and sends the result into `sub.tx`. The CLI supplies `rerun` by calling the **same data-route path** as `/__data` (`route_data_response`) with the stored headers, so middleware, locals, layout loads and gating behave identically (no second implementation). If the rerun redirects or errors, send `event: stale` and close so the client does a normal navigation. 6. **Revalidation hook.** In the revalidate path (`TieredCache::invalidate_tag` call sites in the server and in action handlers' `revalidate.tag`), after the cache work call `hub.affected(tag)` and enqueue each. Cross-node: when a Redis handler is active (A10) reuse its pub/sub channel; each node enqueues its own subscribers. 7. **Client.** `packages/oxide/src/client/live.ts`:

```ts
import { decodeOdfOrPatch } from './odf-decode'
import { buildId } from './build'

export function connectLive(route: string, apply: (data: unknown) => void, opts: { poll?: boolean } = {}) {
  let last: unknown, es: EventSource | undefined, timer: ReturnType<typeof setTimeout> | undefined, closed = false
  const open = () => {
    if (closed) return
    const url = '/__oxide/live?route=' + encodeURIComponent(route) + '&url=' + encodeURIComponent(location.pathname + location.search) + '&build=' + encodeURIComponent(buildId() ?? '')
    es = new EventSource(url)
    es.addEventListener('data', (e) => { last = decodeOdfOrPatch((e as MessageEvent).data, last); apply(last) })
    es.addEventListener('stale', () => { es?.close(); location.reload() })
    es.onerror = () => { es?.close(); timer = setTimeout(open, 1000 + Math.random() * 2000) }   // jittered reconnect; server sends a fresh snapshot
  }
  if (opts.poll) { /* poll fallback: fetch /__data?route=... every 5s with If-None-Match */ }
  open()
  return () => { closed = true; es?.close(); if (timer) clearTimeout(timer) }
}
```

`apply` is the router's existing in-place data update (the function HMR's `ox:data` uses); the router calls `connectLive` after hydration when `__oxide.live` is true and disposes on navigation. Respect `document.visibilityState`: close when hidden for more than 30 s, reconnect on `visibilitychange`. 8. **Config.** `live: { maxSubscribers: 5000, debounceMs: 50, keepaliveSeconds: 20, maxRerunsPerSecond: 20, transport: 'sse' | 'poll' }`. 9. **Capability row `Live`:** Full on Binary, Node, Docker; Degraded on serverless (forces `transport: 'poll'`); Unsupported on Static (the build refuses `live: true` there with `AST-LIV-001`).

**Tests.** Hub: `affected_returns_only_subscribers_of_tag`; `guard_drop_unsubscribes`; `capacity_exceeded_is_err`; `wildcard_tag_matches_prefix`; `sse_frame_multiline_data`. Runner: debounce collapses 5 notifications in 50 ms into one rerun (paused time); rate cap; `watch` keeps only the latest snapshot. TS: fake `EventSource` receives data then stale. Integration script `tests/live.sh`: start the smoke app with a live route; `curl -N` two clients; `POST /_oxide/revalidate` for the tag; both streams show `event: data` within 200 ms; the second request's cookie-gated data differs per client. **Security:** per-IP subscriber cap (default 20); cookie snapshot never logged (`is_secret_key`); closing the stream drops the snapshot; Origin check mandatory.

**Acceptance.** Two browser tabs on a live route update within 200 ms of one `revalidate.tag`; killing the server and restarting it reconnects without user action; capability row present.

### V2-F06 ODF PATCH deltas (M, depends on F05)

**Decision refinement (D30).** Rather than a new value tag, a patch is a distinct payload with magic `ODP1` followed by an ordinary ODF1 payload whose root value is `{ b: <base id>, o: [ops] }`. This reuses the existing encoder and decoder byte for byte; only the new magic is added.

**Types and algorithm** (`crates/odf/src/patch.rs`):

```rust
use crate::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Seg { Key(String), Idx(usize) }

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Set { path: Vec<Seg>, value: Value },
    Del { path: Vec<Seg> },
    Splice { path: Vec<Seg>, at: usize, remove: usize, insert: Vec<Value> },
}

/// Structural diff. Objects recurse by key; arrays use common-prefix and common-suffix trimming, so
/// append, prepend, middle-replace and truncate become one Splice; everything else is a Set.
pub fn diff(old: &Value, new: &Value) -> Vec<Op> { let mut ops = Vec::new(); walk(&mut Vec::new(), old, new, &mut ops); ops }

fn walk(path: &mut Vec<Seg>, old: &Value, new: &Value, out: &mut Vec<Op>) {
    if old == new { return; }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                path.push(Seg::Key(k.clone()));
                match a.get(k) { Some(ov) => walk(path, ov, v, out), None => out.push(Op::Set { path: path.clone(), value: v.clone() }) }
                path.pop();
            }
            for k in a.keys().filter(|k| !b.contains_key(*k)) { let mut p = path.clone(); p.push(Seg::Key(k.clone())); out.push(Op::Del { path: p }); }
        }
        (Value::Array(a), Value::Array(b)) => {
            let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
            let (ra, rb) = (a.len() - pre - suf, b.len() - pre - suf);
            if ra > 0 || rb > 0 { out.push(Op::Splice { path: path.clone(), at: pre, remove: ra, insert: b[pre..pre + rb].to_vec() }); }
        }
        _ => out.push(Op::Set { path: path.clone(), value: new.clone() }),
    }
}

pub fn apply(base: &Value, ops: &[Op]) -> Result<Value, String> {
    let mut v = base.clone();
    for op in ops {
        match op {
            Op::Set { path, value } => { *slot(&mut v, path, true)? = value.clone(); }
            Op::Del { path } => {
                let (last, parent) = path.split_last().ok_or("empty path")?;
                match (slot(&mut v, parent, false)?, last) {
                    (Value::Object(m), Seg::Key(k)) => { m.remove(k); }
                    _ => return Err("del target is not an object key".into()),
                }
            }
            Op::Splice { path, at, remove, insert } => match slot(&mut v, path, false)? {
                Value::Array(a) if *at + *remove <= a.len() => { a.splice(*at..*at + *remove, insert.iter().cloned()); }
                _ => return Err("splice target is not a long-enough array".into()),
            },
        }
    }
    Ok(v)
}

fn slot<'a>(v: &'a mut Value, path: &[Seg], create: bool) -> Result<&'a mut Value, String> {
    let mut cur = v;
    for (i, s) in path.iter().enumerate() {
        cur = match (cur, s) {
            (Value::Object(m), Seg::Key(k)) => {
                if create && i == path.len() - 1 && !m.contains_key(k) { m.insert(k.clone(), Value::Null); }
                m.get_mut(k).ok_or_else(|| format!("missing key {k}"))?
            }
            (Value::Array(a), Seg::Idx(n)) => a.get_mut(*n).ok_or_else(|| format!("missing index {n}"))?,
            _ => return Err("path does not match value shape".into()),
        };
    }
    Ok(cur)
}
```

Encoding: `ops_to_value(base_id, ops) -> Value::Object{ b: String, o: Array[ Array[ Number(op code 0|1|2), Array(path: String|Number), ... ] ] }`, then `b"ODP1"` plus `odf::encode(&value)` minus its own `ODF1` magic is **not** done: keep the full ODF1 payload so the existing decoder works unchanged, with `ODP1` as an outer 4-byte prefix. `base_id` is the 16-hex blake3 of the **full ODF bytes of the base** (the client stores the base bytes it last decoded and hashes them with a tiny JS blake3-free fallback: use `sha-256` Web Crypto truncated to 16 hex, and have the server compute the same truncation of the same bytes with `sha2`; do not use blake3 on the client).

**Sending policy.** The server keeps, per live subscriber, the last full encoded snapshot (`Bytes`) and its id. On a rerun: `ops = diff(old, new)`; encode the patch; if `patch.len() * 2 < full.len()` and `ops.len() <= 64`, send `ODP1`; otherwise send the full payload. The client applies a patch only when `b` equals its stored base id; on mismatch it reconnects (fresh snapshot).

**TS** (`packages/oxide/src/client/odf-decode.ts` additions): `decodeOdfOrPatch(b64, last)` sniffs the first four bytes (`ODP1` versus `ODF1`), implements `applyPatch(base, ops)` with the same semantics as the Rust `apply`, and rejects prototype keys (`__proto__`, `constructor`) in paths. **Cross-language golden test:** `crates/odf/src/fixtures.rs::write_all` emits `patch_*.bin` plus the expected result JSON for 12 cases (set, nested set, delete, append, prepend, middle splice, truncate, type change, Map untouched, deep array of objects, no-op, base mismatch); both Rust and TS tests load them. Property test: `apply(a, diff(a, b)) == b` for random `Value` trees (generate objects and arrays only; other kinds go through `Set`).

**Acceptance.** A 2,000-row list with one appended row sends a patch under 5% of the full payload (assert in a test); mismatched base falls back cleanly; golden files byte-identical across implementations.

### V2-F07 Resumable streams (M, no dependencies)

**Goal.** A long streaming response (an LLM-style endpoint, a big export) survives a dropped connection: the client reattaches and receives only the bytes it missed.

**Opt-in.** A `route.ts` handler or server function marks a response resumable: `return resumable(new Response(stream))` (helper in `oxide/server` that sets `x-oxide-resumable: 1`). Only idempotent GET handlers and server functions are eligible; never HTML page streams.

```rust
// crates/server/src/stream_store.rs
use bytes::Bytes;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

pub struct StreamBuf { chunks: Mutex<Vec<Bytes>>, len: AtomicU64, done: AtomicBool, overflow: AtomicBool, notify: Notify, born: Instant }

pub enum Reattach { Gone, Ok(Arc<StreamBuf>) }

pub struct StreamStore { map: dashmap::DashMap<String, Arc<StreamBuf>>, ttl: Duration, max_each: u64 }

impl StreamStore {
    pub fn new(ttl: Duration, max_each: u64) -> Self { Self { map: Default::default(), ttl, max_each } }
    pub fn begin(&self, id: String) -> Arc<StreamBuf> {
        let b = Arc::new(StreamBuf { chunks: Mutex::new(vec![]), len: AtomicU64::new(0), done: AtomicBool::new(false), overflow: AtomicBool::new(false), notify: Notify::new(), born: Instant::now() });
        self.map.insert(id, b.clone()); b
    }
    pub fn append(&self, b: &StreamBuf, chunk: Bytes) {
        if b.overflow.load(Ordering::Acquire) { return; }
        if b.len.load(Ordering::Acquire) + chunk.len() as u64 > self.max_each { b.overflow.store(true, Ordering::Release); b.chunks.lock().unwrap().clear(); return; }
        b.len.fetch_add(chunk.len() as u64, Ordering::AcqRel);
        b.chunks.lock().unwrap().push(chunk);
        b.notify.notify_waiters();
    }
    pub fn finish(&self, b: &StreamBuf) { b.done.store(true, Ordering::Release); b.notify.notify_waiters(); }
    pub fn get(&self, id: &str) -> Reattach {
        match self.map.get(id) { Some(b) if b.born.elapsed() <= self.ttl && !b.overflow.load(Ordering::Acquire) => Reattach::Ok(b.clone()), _ => Reattach::Gone }
    }
    /// Remove expired buffers (called from the 30 s housekeeping task).
    pub fn sweep(&self) { self.map.retain(|_, b| b.born.elapsed() <= self.ttl); }
}

/// Bytes at or after `offset`, concatenated from the held chunks.
pub fn read_from(b: &StreamBuf, offset: u64) -> Option<Bytes> {
    let chunks = b.chunks.lock().unwrap();
    let mut pos = 0u64; let mut out = Vec::new();
    for c in chunks.iter() {
        let end = pos + c.len() as u64;
        if end > offset { let from = offset.saturating_sub(pos) as usize; out.extend_from_slice(&c[from..]); }
        pos = end;
    }
    if offset > pos { return None; }
    Some(Bytes::from(out))
}
```

**Wiring.** (1) After an endpoint or server-fn response arrives with `x-oxide-resumable: 1` and a stream body, generate an id (`blake3(request_id, counter)` 16 hex), add `x-oxide-stream: <id>` to the response, and wrap the body stream in a tee that calls `append` per chunk and `finish` at end (and on error mark done with an `x-oxide-stream-error` trailer-equivalent: a final sentinel is not needed, `done` plus length suffices). (2) New route `GET /__oxide/stream/<id>` (data plane): requires `Range: bytes=<n>-` (default 0). If `Gone` or the offset is greater than `len`, respond `410`. Otherwise `206` with `content-range: bytes n-*/*` and a stream that first emits `read_from(b, n)`, then waits on `notify` for new chunks until `done`. A completed buffer with `n == len` returns `200`, empty body, `x-oxide-stream-complete: 1`. (3) Limits: `stream.resume: { ttl: '60s', maxBytesEach: '4mb', maxBytesTotal: '256mb' }` (config, schema entry); total accounting evicts oldest completed buffers first; per-IP reattach rate limit.

**Client** (`packages/oxide/src/client/resume.ts`):

```ts
export async function resumableFetch(input: RequestInfo, init?: RequestInit, onChunk?: (c: Uint8Array) => void) {
  const res = await fetch(input, init)
  const id = res.headers.get('x-oxide-stream')
  if (!id || !res.body) return res
  let got = 0
  async function pump(r: Response) {
    const rd = r.body!.getReader()
    try { for (;;) { const { done, value } = await rd.read(); if (done) return true; got += value.length; onChunk?.(value) } }
    catch { return false }
  }
  let ok = await pump(res)
  for (let tries = 0; !ok && tries < 5; tries++) {
    await new Promise(r => setTimeout(r, 200 * 2 ** tries))
    const r2 = await fetch('/__oxide/stream/' + id, { headers: { Range: 'bytes=' + got + '-' } })
    if (r2.status === 410) throw new Error('stream expired')
    ok = await pump(r2)
  }
  if (!ok) throw new Error('stream lost')
  return res
}
```

**Tests.** `read_from` offsets (start, mid-chunk, end, beyond end is None); overflow disables replay; TTL expiry returns `Gone`; reattach after complete yields the tail; concurrent append and read; TS: fake fetch drops mid-stream and the second call receives `Range: bytes=<n>-` with the exact count. E2E: an endpoint emits 20 numbered lines at 50 ms; kill the first connection after 8 lines (`curl --max-time`); reattach with the byte offset; the concatenation equals the full stream byte for byte. **Acceptance:** that e2e passes; memory stays under `maxBytesTotal` in a 1,000-stream soak.

### V2-F12 Brownout mode (M, depends on B03)

**Goal.** Under overload, degrade deliberately (stale content, low-priority shedding) instead of timing out everywhere.

**Design.** A `Governor` computes a level from observable signals every 500 ms; the request path reads one atomic.

```rust
// crates/server/src/governor.rs
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level { Normal = 0, Stale = 1, Shed = 2, Emergency = 3 }

pub struct Signals { pub queue_wait_p99_us: u64, pub loop_lag_us: u64, pub inflight: u32, pub rss_mb: u64 }

pub struct Thresholds { pub queue_wait_us: [u64; 3], pub loop_lag_us: [u64; 3], pub rss_mb: [u64; 3], pub exit_ratio: f64, pub calm_ticks: u32 }

impl Default for Thresholds {
    fn default() -> Self { Self { queue_wait_us: [50_000, 200_000, 500_000], loop_lag_us: [20_000, 100_000, 300_000], rss_mb: [u64::MAX; 3], exit_ratio: 0.6, calm_ticks: 6 } }
}

fn raw_level(s: &Signals, t: &Thresholds) -> u8 {
    (0..3).rev().find(|&i| s.queue_wait_p99_us >= t.queue_wait_us[i] || s.loop_lag_us >= t.loop_lag_us[i] || s.rss_mb >= t.rss_mb[i]).map(|i| i as u8 + 1).unwrap_or(0)
}

/// Pure transition: escalate immediately, de-escalate one level after `calm_ticks` calm ticks (hysteresis).
pub fn next_level(cur: u8, s: &Signals, t: &Thresholds, calm: &mut u32) -> u8 {
    let raw = raw_level(s, t);
    if raw > cur { *calm = 0; return raw; }
    if cur == 0 { return 0; }
    let i = (cur - 1) as usize;
    let calm_now = (s.queue_wait_p99_us as f64) < t.queue_wait_us[i] as f64 * t.exit_ratio && (s.loop_lag_us as f64) < t.loop_lag_us[i] as f64 * t.exit_ratio;
    if calm_now { *calm += 1; if *calm >= t.calm_ticks { *calm = 0; return cur - 1; } } else { *calm = 0; }
    cur
}

pub struct Governor { level: AtomicU8 }
impl Governor {
    pub fn new() -> Self { Self { level: AtomicU8::new(0) } }
    pub fn level(&self) -> Level { match self.level.load(Ordering::Relaxed) { 0 => Level::Normal, 1 => Level::Stale, 2 => Level::Shed, _ => Level::Emergency } }
    pub fn set(&self, l: u8) { self.level.store(l, Ordering::Relaxed); }
}
```

Signals: `pool.stats()` gives queue wait and in-flight (PERF-02 gauges exist); loop lag is measured by a task that sleeps 100 ms and records the overshoot; RSS from `/proc/self/statm` or `sysinfo` (already used by doctor, verify). The sampler task runs `next_level` and calls `set`, and updates gauges `oxide_brownout_level`.

**Enforcement** (stage after prelude): level at least `Stale`: the L0 lookup may serve entries past their SWR window (up to `brownout.staleCeiling`, default 10x TTL) with header `warning: 110 - "Response is Stale"`, while a single-flight regeneration proceeds. Level `Shed`: routes with `priority: 'low'` get `503` with `retry-after: 5`, `x-oxide-brownout: 2`; `normal` routes keep serving; `critical` always serve. Level `Emergency`: only L0 hits, static assets and `critical` routes; everything else `503`. Priority is declared in `page.server.ts` (`config.priority`) or config `rules` (`{ source: '/api/export', priority: 'low' }`), default `normal`, stored in the route manifest. Health: `/_oxide/health` stays 200 (liveness) and reports the level; `/_oxide/ready` returns 503 only at `Emergency` so a balancer can drain.

**Hedging (opt-in, off by default).** `brownout.hedge: { enabled, afterFactor: 2.0 }`: for idempotent GET renders still running after `afterFactor` times the route's recent p95, if another isolate is idle, start the same render there and take the first completed; cancel the loser. Only for `dynamic` GET without `set-cookie` in the response so far. Document the idempotency requirement.

**Tests.** `next_level` table (escalates immediately; needs 6 calm ticks to drop; hysteresis band; never skips below 0), enforcement unit tests with a fake governor level per priority and route, header assertions, readiness only at Emergency. Load test (`benchmarks/overload.sh`): `oha -c 500` against a slow dynamic route with `critical` L0 route alongside: the critical route's p99 stays within 2x its idle p99 while the slow route sheds with 503. **Acceptance:** that script passes; at `Normal` the added per-request cost is one relaxed atomic load (assert via the B09 allocation test: no new allocations).

### V2-F08 Native Rust server functions (M, depends on R02)

**Goal.** A `*.server.rs` file exports typed functions callable from any adapter's client code, with no JS hop.

**Decision (D33).** Model A only in v0.2: the user's Rust crate is built as a `cdylib` and loaded with `libloading`; model B (static link into the server binary) is deferred. The calling convention is a C ABI over ODF bytes, so no Rust types or allocators cross the library boundary.

**Files.** New crates `crates/macros` (proc macro `oxide::server_fn`) and `crates/rustfn` (host side: loader, registry); `compiler` gains `.server.rs` import stubs; `adapters` gains capability `RustFns`.

```rust
// user code: src/thumb.server.rs
use oxide_rustfn::prelude::*;

#[derive(Deserialize, TsType)] pub struct ThumbIn { pub url: String, pub width: u32 }
#[derive(Serialize, TsType)]   pub struct ThumbOut { pub bytes: Vec<u8>, pub mime: String }

#[oxide::server_fn]
pub fn thumbnail(input: ThumbIn) -> Result<ThumbOut, ServerError> { /* image work, no JS involved */ }
```

```rust
// crates/rustfn/src/abi.rs (shared by the macro output and the host)
#[repr(C)] pub struct RawBuf { pub ptr: *mut u8, pub len: usize, pub cap: usize }
#[repr(C)] pub struct FnEntry { pub name: *const std::os::raw::c_char, pub call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32 }
#[repr(C)] pub struct FnTable { pub abi_version: u32, pub len: usize, pub entries: *const FnEntry, pub free: extern "C" fn(RawBuf) }
pub const ABI_VERSION: u32 = 1;
// the cdylib exports: #[no_mangle] pub extern "C" fn oxide_fn_table() -> *const FnTable
```

The macro generates, per function, an `extern "C"` shim that decodes the ODF input (via `serde` + `oxide_odf`, exposed through `serde_json` fallback if ODF serde support is not present), calls the function, encodes `Ok`/`Err` into the output buffer (status `0` or `1`), and registers it in a generated static `FnTable`. `oxide_fn_table` and `free` are generated once per crate by `oxide_rustfn::export_table!()`. Panics are caught with `catch_unwind` and mapped to status `2` (never unwind across the boundary).

**Host.** `RustFnHost::load(path)`: copy the library to `.oxide/rust/lib-<n>.<ext>` (never load the file cargo writes; Windows locks it), `libloading::Library::new`, read `oxide_fn_table`, check `abi_version == ABI_VERSION` (`RFN-001` on mismatch), index entries by name. A call runs on `spawn_blocking` with a per-function timeout (default 30 s) and a bounded semaphore; results go through the normal ODF response path. **Wire-up with existing server functions:** the id scheme `/_oxide/fn/<hash(file,export,secret)>` is unchanged; the stub generator emits for `.server.rs` exports the same `serverFn`-style client stub as for `.server.ts`, so client code and the §9.4 security checks (POST only, origin, CSRF, build header) are shared. The server function dispatcher (`build_server_fn_dispatcher`) consults the Rust host registry before the JS pool.

**Types.** `#[derive(TsType)]` (use the `ts-rs` crate behind the derive) writes declarations to `.oxide/types/rust-fns.d.ts`, so `thumbnail(input)` is typed in TSX, Vue, Svelte and Solid. **Dev loop:** `oxide dev` watches `**/*.server.rs` and `Cargo.toml`, runs `cargo build --lib` for the user crate (its `[lib] crate-type = ["cdylib"]` is scaffolded by `oxide add rust`), and hot-swaps by loading the new copy and retiring the old `Arc<Library>` after in-flight calls finish. **Production:** `oxide build` builds release, writes `dist/native/<lib>`, and records its hash in the build id inputs (A04); the binary adapter loads it from beside itself. Matrix: Full on Binary and Docker; Unsupported on Node, serverless, edge, static (the build refuses, `RFN-002`, naming the adapter).

**Tests.** Build a fixture crate in the test (`crates/rustfn/tests/fixture`, built with `cargo build` at test time, `#[ignore]` in the quick profile): call success, error mapping, panic maps to status 2 and the host survives, ABI mismatch refused, hot-swap serves new behavior without dropping an in-flight call. **Acceptance:** the smoke app calls `thumbnail` from a React button and from a Vue island; the dev loop reflects a function body change in under 3 s.

### V2-F09 Built-in search with `tantivy` (M, depends on R02)

**Goal.** `oxide build` indexes content collections; the server answers queries natively; static exports ship a compact JSON index with a tiny client matcher.

**Files.** `crates/search/{Cargo.toml,src/lib.rs,src/schema.rs,src/build.rs,src/query.rs}` (feature `search`, default off in dev builds, on in release); `packages/oxide/src/search.ts`; hook in `emit/search.rs` (it already emits a JSON index, this WP supersedes it when the feature is on).

```rust
// crates/search/src/schema.rs (tantivy API names reflect 0.2x; follow cargo doc for the pinned version)
use tantivy::schema::{Schema, STORED, STRING, TEXT, TextFieldIndexing, TextOptions, IndexRecordOption};

pub struct Fields { pub id: Field, pub url: Field, pub title: Field, pub body: Field, pub tags: Field, pub lang: Field }

pub fn build_schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let text = TextOptions::default().set_indexing_options(TextFieldIndexing::default().set_tokenizer("default").set_index_option(IndexRecordOption::WithFreqsAndPositions)).set_stored();
    let f = Fields {
        id: b.add_text_field("id", STRING | STORED), url: b.add_text_field("url", STRING | STORED),
        title: b.add_text_field("title", text.clone()), body: b.add_text_field("body", text),
        tags: b.add_text_field("tags", STRING | STORED), lang: b.add_text_field("lang", STRING | STORED),
    };
    (b.build(), f)
}
```

**Build.** For each collection in `search.collections`, take entries (typed frontmatter and body text with Markdown stripped), index `title` with boost 3.0 and `body` with 1.0 at query time, write the index to `dist/search/index/` (mmap-able) and the static fallback `dist/search/static.json` (`{ docs: [{id,url,title,excerpt,tags}], postings: { term: [docIdx,...] } }`, prefix terms of length 2 to 4 included, gzip-friendly). The build is CAS-cached by the content hash of the collection.

**Serve.** `GET /_oxide/search?q=<text>&limit=10&tag=<t>&lang=<l>` (ops plane) returns `{ total, hits: [{ id, url, title, snippet (HTML with <mark>), score }] }`; the query parser searches `title` and `body` with fuzzy distance 1 on terms of length 5 or more; snippets via tantivy's snippet generator; results cached in L1 for 60 s keyed by the normalized query. Rate limited per IP (default 20 per 10 s). Opening the index at start is a read-only mmap; no writer exists at runtime.

**Client.** `import { search } from 'oxide/search'` returns `search(q, opts) => Promise<Hit[]>`; it calls the endpoint when `window.__oxide.search === 'server'` and otherwise loads `static.json` lazily and matches prefix terms with a ten-line scorer (term hits weighted, title boost) so static sites work with no server.

**Config.** `search: { collections: ['posts'], fields: { title: 3, body: 1 }, snippet: 160, languages: ['en'] }`. Matrix `Search`: Full on Binary, Docker; Degraded (static JSON) on Node, serverless, static; Unsupported on edge until a WASM build exists.

**Tests.** Index three docs and assert ranking (title match beats body match), tag filter, fuzzy match, snippet contains `<mark>`; endpoint integration with the in-process server; static matcher parity test (same corpus, top-3 equal to the server for 20 queries, allowing tie order differences); empty query returns 400. **Acceptance:** 5,000-document corpus builds in under 5 s and answers p99 under 5 ms (criterion bench `search/query`).

### V2-F11 Delta deploys (M, depends on A04, R02)

**Goal.** `oxide deploy` uploads only what changed and swaps atomically.

**Design.** `oxide deploy <target>` where target is `s3://bucket/prefix`, `r2://...` (S3 API) or `dir:/path`. Reuse `object_store` (A10 feature `s3`).

```rust
// crates/adapters/src/delta.rs  (pure planning, no I/O)
use std::collections::BTreeMap;

pub struct Plan { pub upload: Vec<String>, pub keep: Vec<String>, pub delete_later: Vec<String>, pub bytes_up: u64 }

/// local/remote map relative path to content hash. Immutable (hashed-name) assets are never deleted
/// during the skew window, so old tabs keep working.
pub fn plan(local: &BTreeMap<String, (String, u64)>, remote: &BTreeMap<String, String>) -> Plan {
    let (mut upload, mut keep, mut bytes_up) = (vec![], vec![], 0u64);
    for (p, (h, sz)) in local {
        if remote.get(p) == Some(h) { keep.push(p.clone()); } else { upload.push(p.clone()); bytes_up += sz; }
    }
    let delete_later = remote.keys().filter(|p| !local.contains_key(*p)).cloned().collect();
    Plan { upload, keep, delete_later, bytes_up }
}
```

**Order of operations (atomicity).** (1) Upload new immutable files (`assets/*`, hashed chunks) first, in parallel (bounded, resumable by hash). (2) Upload non-hashed files (HTML, manifests) next, but under a new prefix `releases/<build-id>/`. (3) Write the pointer `current` (a 40-byte object containing the build id) **last**: the switch is one small write. (4) After `deploy.skewWindow`, a second invocation (`oxide deploy --prune`) deletes `delete_later` entries and old `releases/*`. Rollback is rewriting `current` to the previous id. Tests: plan table (new, changed, unchanged, removed), idempotent re-run uploads nothing, a failure mid-upload leaves `current` untouched (fake store with an injected error), prune keeps files younger than the window. **Acceptance:** second deploy after a one-line change uploads under 5% of the bytes.

### V2-F13 Learned prefetch (M, depends on A04)

**Goal.** Prefetch the link the visitor is most likely to follow, using aggregate counts only.

**Design.** (1) The client router adds `x-oxide-nav-from: <route id>` to `/__data` fetches it issues for navigations. (2) The server updates `NavGraph`:

```rust
// crates/server/src/navgraph.rs
pub struct NavGraph { edges: dashmap::DashMap<(String, String), f64>, half_life_s: f64, last_decay: std::sync::Mutex<std::time::Instant> }
impl NavGraph {
    pub fn observe(&self, from: &str, to: &str) { *self.edges.entry((from.into(), to.into())).or_insert(0.0) += 1.0; }
    pub fn decay(&self, now: std::time::Instant) {
        let mut last = self.last_decay.lock().unwrap();
        let f = 0.5f64.powf(now.duration_since(*last).as_secs_f64() / self.half_life_s);
        for mut e in self.edges.iter_mut() { *e.value_mut() *= f; }
        *last = now;
    }
    /// Top k next routes with probability; only edges whose weight is at least `k_anon` are exposed.
    pub fn top(&self, from: &str, k: usize, k_anon: f64) -> Vec<(String, f32)> {
        let mut v: Vec<(String, f64)> = self.edges.iter().filter(|e| e.key().0 == from).map(|e| (e.key().1.clone(), *e.value())).collect();
        let total: f64 = v.iter().map(|x| x.1).sum();
        v.retain(|x| x.1 >= k_anon);
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        v.into_iter().take(k).map(|(r, w)| (r, (w / total) as f32)).collect()
    }
}
```

(3) `GET /_oxide/nav?from=<route>` returns the top 3 (`k_anon` default 20, half-life 7 days, flushed to `.oxide/nav-graph.json` hourly and loaded at start). No user identifiers are stored or sent; edges are route ids, never URLs with params. (4) The client, after idle (`requestIdleCallback`) and when `navigator.connection.saveData` is false and `effectiveType` is not `2g`, prefetches data and chunks for edges with probability at least 0.35 (max 3 outstanding, one new prefetch per 2 s, cancelled on navigation). Chromium also gets an inline `<script type='speculationrules'>` for the top edge. Config `prefetch: { learned: true, minProbability: 0.35 }`. Tests: observe and top ranking, decay halves weights after one half-life, k-anonymity hides sparse edges, probability sums, TS policy with fake connection objects. **Acceptance:** in a synthetic replay of 1,000 sessions the top edge matches the generator's dominant transition.

### V2-F14 Offline-first module (M, depends on A04)

**Design.** Extend the PWA worker (`emit/pwa.rs`) with versioned caches named by build id: precache hashed assets and the shell from the manifest; `/__data` responses cached stale-while-revalidate; navigation falls back to the cached route shell or `/offline`. Add the **outbox** for actions:

```ts
// packages/oxide/src/client/outbox.ts
type Item = { id: string; url: string; method: string; body: string; headers: Record<string, string>; tries: number }
const DB = 'ox-outbox'
async function db(): Promise<IDBDatabase> {
  return new Promise((res, rej) => { const r = indexedDB.open(DB, 1); r.onupgradeneeded = () => r.result.createObjectStore('q', { keyPath: 'id' }); r.onsuccess = () => res(r.result); r.onerror = () => rej(r.error) })
}
export async function enqueue(it: Omit<Item, 'id' | 'tries'>) {
  const id = crypto.randomUUID()
  const d = await db()
  await new Promise<void>((res, rej) => { const t = d.transaction('q', 'readwrite'); t.objectStore('q').put({ ...it, id, tries: 0, headers: { ...it.headers, 'x-oxide-idem': id } }); t.oncomplete = () => res(); t.onerror = () => rej(t.error) })
  return id
}
export async function flush(send: (i: Item) => Promise<Response> = (i) => fetch(i.url, { method: i.method, headers: i.headers, body: i.body })) {
  const d = await db()
  const items: Item[] = await new Promise((res) => { const q = d.transaction('q').objectStore('q').getAll(); q.onsuccess = () => res(q.result as Item[]) })
  for (const it of items) {
    try {
      const r = await send(it)
      if (r.ok || (r.status >= 400 && r.status < 500 && r.status !== 408 && r.status !== 429)) await del(d, it.id)   // delivered or permanently rejected
      else throw new Error('retry')
    } catch { /* keep for the next flush */ }
  }
}
async function del(d: IDBDatabase, id: string) { await new Promise<void>((res) => { const t = d.transaction('q', 'readwrite'); t.objectStore('q').delete(id); t.oncomplete = () => res() }) }
window.addEventListener('online', () => void flush())
```

**Server idempotency.** A stage `stage_idempotency` for POST actions and server functions: if `x-oxide-idem` is present, look up `idem:<key>` in L1; a hit returns the stored response (status, headers minus `set-cookie`, body); a miss runs the handler and stores the result for 24 h (only for 2xx and 4xx). Tests: replay of the same key returns identical bytes and the handler runs once; different keys run separately; outbox flush keeps 5xx items and deletes 4xx; offline `enhance` enqueues. Config `offline: { enabled, precache: 'routes' | 'all', outbox: true }`; matrix `Offline` Full wherever a browser is the client. **Acceptance:** Playwright offline mode: submit the form, go online, exactly one server-side insert.

### V2-F15 Record and replay (M, depends on A05)

**Goal.** `oxide replay <id>` re-executes a captured request deterministically.

**Recorder** (dev always when `record: 'errors' | 'all'`, prod sampled `record: { sample: 0.001, onError: true }`): store `.oxide/records/<request-id>.json` with: request (method, URL, headers with `authorization`, `cookie`, `set-cookie` replaced by `sha256:<12 hex>` unless `--include-secrets` in dev), matched route id and params, middleware locals, each loader input and ODF output, each outbound `fetch` op call and response (status, headers, body base64, up to 1 MB), cache op results, the build id, and the values returned by non-deterministic sources: `Date.now()`, `Math.random()` (seeded stream), `crypto.randomUUID()`.

**Replay.** The worker installs `globalThis.__oxide_replay` when started with `OXIDE_REPLAY=<file>`: it overrides `fetch` with the recorded responses (matched by method, URL and body hash in order; an unexpected call is an error `RPL-002` naming the call), `Date.now`, `Math.random` and `crypto.randomUUID` from the recorded sequences, then runs the request through the normal pipeline. `oxide replay <id> [--diff] [--json]` prints status, headers and body, and with `--diff` compares to the recorded response (normalizing the nonce). For agents, `--json` emits `{ ok, divergences: [..] }`. Tests: record then replay a page with a `fetch` and a random id yields byte-identical HTML; an extra fetch fails with `RPL-002`; redaction replaces cookie values; a record from an older build id warns (`RPL-003`) but runs. **Acceptance:** a failing loader in the smoke app is reproducible from its record without network access.

### V2-F17 Component cost overlay (M, depends on E01)

Dev-only (`?ox-cost` or Alt+Shift+O). **Data available per adapter:** bundle size per source module for all four (Rolldown module `renderedLength` mapped to files, gzip estimated by compressing each module's rendered code), server render time per component only for React (the dev build wraps route components in `<Profiler>`) and Vue (`app.config.performance`); Svelte and Solid show bytes only, and the overlay says so. **Mapping DOM to source:** in dev, the compiler adds `data-ox-src='path:line'` to the first element of each component (React and Solid through an Oxc visitor on JSX; Vue and Svelte through their compile hooks). `GET /__oxide/cost?src=<path>` (dev-token guarded, D41) returns `{ bytes, gzip, chunk, renderMsP50, cache }`. The overlay highlights the hovered element's component and lists the three heaviest. Tests: attribute injection snapshot for React and Vue fixtures; endpoint returns sizes that sum to the chunk size within 2%. **Acceptance:** hovering a component in the React smoke app shows its byte cost; the Svelte smoke app shows bytes with a "render time unavailable" note.

### V2-F18 Performance bisect (S, depends on B09)

`oxide perf bisect --good <rev> --bad <rev> --metric build-ms|client-js-bytes|ttfb-ms [--threshold 10%] [--runs 3]`. The search is a pure function so it is testable without git:

```rust
pub fn bisect<F: FnMut(&str) -> f64>(commits: &[String], mut measure: F, good: f64, threshold_pct: f64) -> Option<usize> {
    // commits ordered oldest (known good) to newest (known bad); returns the index of the first commit whose metric exceeds good*(1+threshold)
    let limit = good * (1.0 + threshold_pct / 100.0);
    let (mut lo, mut hi) = (0usize, commits.len().checked_sub(1)?);
    if measure(&commits[hi]) <= limit { return None; }   // the bad end does not regress
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if measure(&commits[mid]) > limit { hi = mid } else { lo = mid + 1 }
    }
    Some(lo)
}
```

The command lists `git rev-list --first-parent <good>..<bad>`, builds each candidate in a `git worktree` under `.oxide/bisect/`, all sharing one `OXIDE_CACHE_DIR` so rebuilds are incremental, measures the median of `--runs`, and prints the culprit commit with the metric series. Tests: `bisect` with monotone and noisy fake measures (median of three), no-regression returns `None`. **Acceptance:** a seeded regression commit in a fixture repo is found in at most `ceil(log2(n))+1` builds.

### V2-F19 Per-route capability permissions (L, spike, depends on A08)

**Question.** Can `net`, `fs` and `env` be enforced per route inside a shared, interleaving isolate? **Declaration:** `config.permissions = { net: ['api.stripe.com', '*.internal'], env: ['STRIPE_KEY'], fs: ['./data'] }` in `page.server.ts`, with `permissions.default` in the config (default allow-all in v0.2, deny-by-default opt-in).

```rust
// crates/jshost/src/perm.rs
#[derive(Clone, Default, Debug)]
pub struct PermissionSet { pub net: Option<Vec<String>>, pub env: Option<Vec<String>>, pub fs: Option<Vec<String>> }   // None = unrestricted

pub fn host_allowed(pat: &[String], host: &str) -> bool {
    pat.iter().any(|p| p == host || p.strip_prefix("*.").map_or(false, |suf| host.ends_with(suf) && host.len() > suf.len() && host.as_bytes()[host.len() - suf.len() - 1] == b'.'))
}
```

**Model 1 (attribute by request):** the dispatcher records `request_id -> PermissionSet` before calling into the isolate; `op_fetch`, `op_env_get` and `op_fs_read` receive the request id from the `AsyncContext` and check it. Risk: attribution is lost across third-party libraries that detach context (timers created outside the context). **Model 2 (isolate per profile):** isolates are pooled by profile hash, so enforcement is per isolate and always sound, at the cost of more isolates. **Spike deliverable** (`docs/decisions/F19-permissions.md`): prototype Model 1 on `op_fetch` and `process.env` (Proxy through an op); run a hostile fixture that creates a `setTimeout` outside the request context and calls `fetch`; if any bypass exists, adopt Model 2 or kill. On Node the enforcement is advisory (matrix says so; D35). **Kill:** a bypass that Model 2 cannot close, or isolate-count growth above 3x for typical apps.

### V2-F20 Attested reproducible builds (S, depends on A04)

`oxide build --attest` writes `dist/attest/`: `sbom.cdx.json` (CycloneDX 1.5 from `Cargo.lock` for the binary plus `pnpm-lock.yaml` for packages that reached a bundle, using our own serde structs, no external generator), `provenance.intoto.json` (in-toto Statement v1; `predicateType` `https://slsa.dev/provenance/v1`; `subject` entries are the SHA-256 of `dist/build-id`, the binary and each top-level artifact; `buildDefinition` has the oxide version, config hash, git commit and dirty flag; `runDetails.builder.id` from `OXIDE_BUILDER_ID` or `local`), and `provenance.sig` (ed25519 over the canonical JSON, key from `OXIDE_SIGNING_KEY` or `--key <file>`; the public key id is recorded). `oxide verify <dist> [--pubkey <file>] [--rebuild]` checks the signature, the subject digests, and with `--rebuild` rebuilds in a temp directory and compares `dist-digest`. **Determinism requirements** (tested): no timestamps in artifacts (`SOURCE_DATE_EPOCH` for any needed time), sorted maps and file lists, path normalization (strip the project root), identical `dist-digest` for two builds in different directories. Tests: SBOM contains known crates; signature round trip; tampering with one byte fails `verify`; two builds equal. **Acceptance:** CI job builds twice in different paths and `verify --rebuild` passes.

## V2.9 Wave E: developer experience and ecosystem

### V2-E01 Complete HMR: boundaries, state preservation, CSS (M, no dependencies)

**Evidence.** `ox:data` (loader HMR) and Fast Refresh plumbing for React and Svelte exist (`crates/dev`, `compiler/src/framework.rs`, `packages/{react,svelte}/src/real-adapter.ts`); Vue and Solid refresh and the boundary algorithm are not specified in code. **Message schema (version 1)**, in `packages/oxide/src/client/hmr.ts` and `crates/dev`: `{ v: 1, type: 'update', updates: [{ kind: 'js'|'css'|'data', path, acceptedPath, timestamp, hash }] } | { type: 'full-reload', reason } | { type: 'error', diagnostic } | { type: 'ox:data', route, data } | { type: 'connected', build }`.

**Boundary algorithm** (pure Rust in `crates/dev/src/boundary.rs`): given the module graph as `importers: HashMap<Path, Vec<Path>>` and `self_accepting: HashSet<Path>` (a module is self-accepting when the adapter's compile step registered `import.meta.hot.accept` or the framework plugin marks component modules as boundaries), find the set of update boundaries for a changed file by walking importers breadth-first until a self-accepting module is reached on every path; if any path reaches an entry module without a boundary, return `FullReload`. Cycle-safe (visited set).

```rust
pub enum Plan { Update(Vec<PathBuf>), FullReload(String) }
pub fn plan(changed: &Path, importers: &HashMap<PathBuf, Vec<PathBuf>>, accepting: &HashSet<PathBuf>, entries: &HashSet<PathBuf>) -> Plan {
    let mut boundaries = Vec::new();
    let mut seen = HashSet::new();
    let mut q = std::collections::VecDeque::from([changed.to_path_buf()]);
    while let Some(m) = q.pop_front() {
        if !seen.insert(m.clone()) { continue; }
        if accepting.contains(&m) { boundaries.push(m); continue; }
        if entries.contains(&m) { return Plan::FullReload(format!("{} has no HMR boundary", m.display())); }
        match importers.get(&m) { Some(v) if !v.is_empty() => q.extend(v.iter().cloned()), _ => return Plan::FullReload(format!("{} is not imported", m.display())) }
    }
    boundaries.sort(); boundaries.dedup();
    Plan::Update(boundaries)
}
```

Tests: self-accepting file updates alone; a util edit bubbles to two component boundaries; an entry without a boundary full-reloads; cycles terminate; orphan module full-reloads. **Per-adapter runtime:** React `react-refresh/runtime` (exists); Vue `__VUE_HMR_RUNTIME__.reload(id, comp)` emitted by the Vue compile step (native or pool; C03 must preserve it); Svelte `svelte-hmr` equivalent (exists in adapter); Solid `solid-refresh` registration emitted for Solid modules (add if absent). **CSS:** `kind: 'css'` swaps the `<link>` or `<style data-ox-css=path>` without reload. **Error recovery:** an `error` message shows the overlay and the next successful `update` removes it and re-applies without reload. **Acceptance (Playwright, E10, all four adapters):** type into an input, edit the component's text, the input value survives; edit a CSS file, no navigation event fires; introduce a syntax error then fix it, overlay appears then disappears with state intact.

### V2-E02 Typed search params, typed `href` and `goto` (M, no dependencies)

**Goal.** SPEC-09's remainder: search params typed from a schema; links typed end to end in all four adapters.

**Design.** A route may export `searchSchema` (any Standard Schema) from `page.server.ts`. Typegen (`crates/router/src/typegen.rs`) cannot compute a TypeScript type from a schema, so it **delegates inference to TypeScript**: it emits `.oxide/types/routes.d.ts` like

```ts
import type { StandardSchemaV1 as S } from '@standard-schema/spec'
import type { searchSchema as s_blog_slug } from '../../src/routes/blog/[slug]/page.server'
import type { load as l_blog_slug } from '../../src/routes/blog/[slug]/page.server'

export interface RouteMap {
  '/blog/[slug]': { params: { slug: string }; search: S.InferOutput<typeof s_blog_slug>; data: Awaited<ReturnType<typeof l_blog_slug>> }
  '/': { params: {}; search: {}; data: {} }
}
```

and generic helpers (shared in `oxide/types`):

```ts
export function href<R extends keyof RouteMap>(route: R, ...a: HrefArgs<R>): string
type HrefArgs<R extends keyof RouteMap> = [RouteMap[R]['params']] extends [Record<string, never>] ? [opts?: { search?: Partial<RouteMap[R]['search']>; hash?: string }] : [params: RouteMap[R]['params'], opts?: { search?: Partial<RouteMap[R]['search']>; hash?: string }]
```

Per-adapter `Link`/`goto` wrap `href` (React generic component, Vue `defineComponent` with typed props via `.d.ts`, Svelte generic component types, Solid generic). Runtime: `parseSearch(schema, url.searchParams)` calls `schema['~standard'].validate` and surfaces `issues` as a 400 page (`OX-SRC-001`) or a redirect to a corrected URL when the schema has defaults; the loader receives `search` typed. Declaring `searchSchema` also tells render-mode inference that listed keys are **cache key dimensions** (not a dynamic read), so F16's `SearchParamRead` is satisfied by declaring the schema. **Tests:** typegen snapshot (insta) for four routes; a `tsc --noEmit` fixture (`tests/types/`) with `// @ts-expect-error` lines proving wrong params, missing params and wrong search types are rejected (run by `oxide check` in CI); runtime parse tests for defaults, coercion and issues. **Acceptance:** the type fixtures compile or fail exactly where annotated, in all four adapter packages.

### V2-E03 Real `oxide migrate` codemods (M, no dependencies)

Default is a dry run that prints a plan; `--apply` requires a clean git tree (or `--force`). The plan is data so it is testable:

```rust
pub struct MigrationPlan { pub renames: Vec<(PathBuf, PathBuf)>, pub rewrites: Vec<ImportRewrite>, pub config: Vec<ConfigKey>, pub manual: Vec<Note> }
pub struct ImportRewrite { pub file: PathBuf, pub from: String, pub to: String }
pub struct Note { pub file: PathBuf, pub line: u32, pub text: String }
```

**SvelteKit** (pure path mapping, highest value, safest): `src/routes/**/+page.svelte` to `page.svelte`, `+layout.svelte` to `layout.svelte`, `+page.server.ts` to `page.server.ts` (its `load` and `actions` are already shaped like Oxide's), `+layout.server.ts`, `+server.ts` to `route.ts`, `+error.svelte` to `error.svelte`, `hooks.server.ts` to a generated `middleware.ts` calling the `handle` export (flag as manual if it uses `sequence`), `$lib` alias to a tsconfig path. **Next.js (app router):** enable Next-compat conventions (`routes.compat: 'next'`), `generateStaticParams` to `generateParams`, `next/link` and `next/image` import rewrites to the adapter's `Link`/`Image`, `'use client'` files listed as island candidates (report, do not guess), `next.config.js` keys mapped by table (`images.remotePatterns`, `redirects`, `rewrites`, `headers`, `i18n`) into `oxide.config.ts`; pages router, middleware matchers with complex regex, and server components using async data in components go to `manual`. **Nuxt:** `pages/` to `routes/` with `[param]` and `[...slug]` conventions, `useFetch` and `useAsyncData` flagged manual with the suggested `load`. **Vite SPA:** wrap existing entry as a `spa` route, add `oxide.config.ts`. Import rewriting uses the Oxc import span scanner (`scan_import_spans`), never regex on source. **Tests:** per source framework a fixture project (`fixtures/migrate/{sveltekit,next,nuxt,vite}`) with a golden plan JSON; applying the plan then running `oxide check` and `oxide build` on the SvelteKit and Vite fixtures must succeed; unsupported constructs appear in `manual` with file and line. **Acceptance:** the SvelteKit fixture builds with zero manual notes beyond `hooks.server.ts` with `sequence`.

### V2-E04 SRI, Trusted Types and CSP report-only (S, no dependencies)

**SRI.** At client bundle end, compute `sha384-<base64>` for every emitted JS and CSS chunk and store it in `client.manifest.json` as `integrity: { file: value }`. The head injector adds `integrity='...' crossorigin` to `<script type=module>`, `<link rel=modulepreload>` and stylesheet links for chunks (not for dev). Content-hashed names plus SRI means a CDN serving a modified chunk fails closed. Config `security.sri: true` (default on in production builds). Test: manifest has the right digest for a fixture chunk (recompute independently in the test); injected tags carry it; a mismatch case documented.

**Trusted Types.** `security.trustedTypes: 'off' | 'report' | 'enforce'`. When on, CSP adds `require-trusted-types-for 'script'; trusted-types oxide default`. The core boot defines the policy once:

```ts
export const tt: { createHTML(s: string): any } = (globalThis as any).trustedTypes?.createPolicy('oxide', { createHTML: (s: string) => s }) ?? { createHTML: (s: string) => s }
```

and every core `innerHTML` assignment (deferred swap, island loader, dev overlay) goes through `tt.createHTML`. A lint test (`rg 'innerHTML' packages/oxide/src` must only match lines using `tt.`) enforces it. **CSP report-only.** `security.csp: 'report-only'` sends `Content-Security-Policy-Report-Only` plus `report-to`, and `POST /_oxide/csp-report` accepts both `application/csp-report` and `application/reports+json`, rate limited (20 per minute per IP), logs a structured event (no cookies), returns 204. Tests: header presence per mode, endpoint parses both formats, oversized body rejected.

### V2-E05 Plugin compatibility harness (M, depends on A09)

**Goal.** A published, CI-verified table of which Rollup and Vite plugins work.

Case format `conformance/plugins/cases/<name>.json`: `{ "plugin": "vite-plugin-svgr@x", "fixture": "svg-component", "adapter": "react", "assert": [{ "file": "dist/client/*.js", "contains": "createElement" }], "hooks": ["transform"] }`. Runner `scripts/plugin-compat.mjs` installs each plugin into a temp copy of the fixture, builds with `oxide-cli build`, evaluates `assert`, and records `{ status: pass|fail|unsupported-hook, unsupportedHooks: [...] }` from the bridge's load-time report. Output `docs/plugin-compat.md` (generated table, checked in CI by `docs-check`, E11). Initial cases (verify each package name and version when writing): the official framework plugins' behavior (`@sveltejs/vite-plugin-svelte`, `@vitejs/plugin-vue`, `vite-plugin-solid`), MDX, SVGR, Tailwind, `unplugin-icons`, `vite-tsconfig-paths`, `vite-plugin-pwa`. **Acceptance:** at least ten third-party plugins verified (release gate item 6 of Part I §24.1), failures documented not hidden.

### V2-E06 Module API v1 and first-party modules (M, no dependencies)

**Evidence.** `plugin/src/lib.rs:251` defines `Module`; `oxide add <module>` only records a name in `.oxide/modules.json`. **API:**

```ts
export interface ModuleContext {
  addRoute(r: { path: string; file: string }): void
  addMiddleware(file: string, opts?: { scope?: string }): void
  addPlugin(p: unknown): void
  addTypes(name: string, dts: string): void
  extendConfig(patch: Record<string, unknown>): void
  addServerFn(file: string): void
  options: unknown
}
export function defineModule<O>(def: { name: string; adapters?: Array<'react' | 'vue' | 'svelte' | 'solid'>; setup(ctx: ModuleContext, options: O): void | Promise<void> }): unknown
```

`oxide add <pkg>` becomes real: detect the package manager from the lockfile (`pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `bun.lockb`), run the install, then edit `oxide.config.ts` with an Oxc AST edit that adds the import and the element in the `modules` array (idempotent; refuses if the config is dynamic, printing the two lines to paste). Modules declare `adapters`; adding one to an unsupported adapter fails with a clear message. **First-party modules (each a package under `packages/modules/`):** `@oxide/tailwind` (native engine wiring, content globs from the graph), `@oxide/analytics` (web-vitals beacon sink with a generic webhook and console targets), `@oxide/errors` (error reporting to a webhook with release tagging by build id), `@oxide/image-cdn` (loaders for Cloudinary and imgix), `@oxide/auth` (sessions and CSRF helpers over the existing session codec, OAuth provider recipes as documented examples, no custom crypto). **Tests:** `setup` receives a recording fake context; config edit is idempotent and preserves comments (round-trip fixtures); each module has a fixture app. **Acceptance:** `oxide add @oxide/tailwind` on the Svelte smoke app yields styled output with no manual edits.

### V2-E07 `llms.txt` and expanded MCP tools (S, no dependencies)

**`llms.txt`.** Config `llms: { enabled: true, title, summary, include: ['/docs/**'] }`. At build, emit `dist/llms.txt` (title, summary, then a link list of included routes with their meta descriptions) and `dist/llms-full.txt` (route text content from content collections, Markdown). Reuse the sitemap route list. **MCP.** The `oxide mcp` server gets a tool registry:

```rust
#[async_trait::async_trait]
pub trait McpTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;           // JSON Schema
    fn writes(&self) -> bool { false }                      // write tools need --allow-write
    async fn call(&self, cx: &McpCx, args: serde_json::Value) -> Result<serde_json::Value, String>;
}
```

Tools: `routes.list` (id, pattern, mode, why-dynamic reasons, cache policy), `route.explain` (layout chain, middleware chain, loads, tags), `errors.recent` (diagnostics sidecar), `cache.inspect` (entries by tag or route), `graph.why` (importers and chunks of a module), `config.get` (resolved, secrets redacted), `build.run` (write, needs `--allow-write`), `replay.run` (F15). Read-only by default; secrets never returned (`is_secret_key`). Tests: `tools/list` returns the registry with schemas; a write tool is refused without the flag; redaction test. **Acceptance:** an MCP client can answer "why is /dashboard dynamic?" using only tool calls.

### V2-E08 Dev experience v2 (M, depends on E01)

1. **Overlay:** source-mapped stack frames, "open in editor" via `GET /__oxide/open?file=&line=` (dev-token guarded, path confined to the project root, editor from `OXIDE_EDITOR` or `$EDITOR`/`code -g`), dismissible and keyboard-accessible, and Rust diagnostics (`miette` JSON) rendered with code frames and the fix line. 2. **Inspector (`/__oxide`):** module graph search and `why`, route table with inferred mode and F16 reasons, cache entries by tag, live subscribers (F05), generations (F10), per-request waterfall (middleware, loads, render, inject), isolate pool state. 3. **`oxide doctor` additions:** native compiler health (C01), cache handler reachability (A10), tool versions (`tsgo`, `oxlint`, `oxfmt`, Node for the NodeHost, a Rust toolchain when `*.server.rs` exists), port availability, file-descriptor limit (`ulimit -n` under 4,096 warns), clock skew for signed cookies. 4. **Terminal output:** one summary line per request in dev with timing split (`match 0.1ms load 12ms render 8ms`), colors only on TTY, `--json` logs for CI. **Tests:** doctor checks are table-driven with fake probes; the open-in-editor endpoint rejects `../` and files outside the root; overlay renders a fixture diagnostic. **Acceptance:** a seeded import error shows file, line, frame and fix in the overlay and in `--json` output.

### V2-E09 Edge completion (M, depends on A08)

**Evidence.** `core-wasm` exports `edge_cache_key`, `merge_head`, `encode_odf`, `decode_odf`, `edge_manifest_routes`; the Cloudflare adapter emits a skeleton. **Work:** add to `core-wasm` the router match over the JSON route tree (reuse `CandidateIndex` from B02, which has no I/O), the native rules engine (redirects, rewrites, headers, i18n negotiation compiled to a table at build), `NonceSplicer`, `KeyStr`, `ODP1` apply (F06) and `sse_frame`. Build with `wasm-bindgen` (target `web`, `wasm-opt -Oz`, budget 400 KB gzip, enforced by `budgets`). Worker entry generated at `dist/edge/worker.js`:

```js
import wasm from './core.wasm'            // module binding
import { init, matchRoute, applyRules } from './core.js'
import { render } from './ssr-edge.js'    // edge-environment SSR bundle built by Rolldown
let ready; export default {
  async fetch(req, env, ctx) {
    ready ??= init(wasm)
    await ready
    const url = new URL(req.url)
    const rule = applyRules(url.pathname, req.headers.get('accept-language') ?? '')
    if (rule?.redirect) return Response.redirect(new URL(rule.redirect, url), rule.status)
    const asset = await env.ASSETS.fetch(req); if (asset.status !== 404) return asset
    const m = matchRoute(url.pathname); if (!m) return new Response('Not found', { status: 404 })
    const key = 'l0:' + m.id + ':' + url.search
    const hit = await env.OXIDE_KV.get(key, 'arrayBuffer'); if (hit) return new Response(hit, { headers: { 'content-type': 'text/html; charset=utf-8', 'x-oxide-cache': 'HIT' } })
    const res = await render(req, m, env); ctx.waitUntil(env.OXIDE_KV.put(key, await res.clone().arrayBuffer(), { expirationTtl: 60 })); return res
  },
}
```

KV cache handler for the edge (tags via a KV index key per tag with a size cap, documented as best effort). **Tests:** Rust `core-wasm` tests mirror the native tests for each exported function (same vectors); a `wrangler dev --local`/`workerd` job in CI renders the React fixture and fetches `/`, a dynamic route, a redirect and a static asset. Matrix rows for edge are updated truthfully (`Live` Unsupported, `RustFns` Unsupported, `Search` Unsupported, `Skew` Degraded). **Acceptance:** the CI job passes; the wasm budget is enforced.

### V2-E10 Playwright automation (M, no dependencies)

`tests/e2e/` with `playwright.config.ts` (projects chromium, firefox, webkit; `workers: 2`; retries 0 for conformance specs, 1 elsewhere), a global setup that builds the four smoke apps once (A01 digest cache reused) and starts each on a random port, and fixtures exposing `app.url`. Specs: `islands-click` (four adapters), `hydration-no-mismatch` (collect `console.error`, fail on any), `nav-prefetch` (hover then click; assert the `/__data` request began before the click), `defer-swap`, `form-no-js` (context with `javaScriptEnabled: false`), `replay` (F03: block the island chunk with `route.abort` then `continue`), `hmr-state` (E01, dev server), `live` (F05, two contexts), `skew` (F10), `offline-outbox` (F14). Example:

```ts
import { test, expect } from '@playwright/test'
test('click before hydration is replayed once', async ({ page }) => {
  let release!: () => void
  const gate = new Promise<void>(r => (release = r))
  await page.route('**/island-*.js', async route => { await gate; await route.continue() })
  await page.goto(process.env.APP_URL!)
  await page.getByRole('button', { name: /count/i }).click()      // SSR button, island not hydrated yet
  release()
  await expect(page.getByTestId('count')).toHaveText('1')          // exactly one, not zero, not two
})
```

CI job `e2e` runs after `build`, caches `~/.cache/ms-playwright`, uploads traces on failure. `pnpm test:e2e` locally. **Acceptance:** all specs green on three browsers for the four adapters (`islands-click` and `hydration-no-mismatch` are in the conformance gate).

### V2-E11 Docs generated from source (M, no dependencies)

`oxide docs generate` writes `docs/reference/`: `config.md` from the JSON Schema (`oxide schema`), `errors.md` from the error code registry (Appendix V2.K, kept as `docs/errors.toml` and checked against `rg 'OxideError|err\("'` call sites by a test), `cli.md` from the `clap` command tree, `capabilities.md` from `oxide_adapters::capability_matrix()`, `plugin-compat.md` (E05), `endpoints.md` (Appendix V2.J). CI job `docs-check` regenerates and fails on `git diff --exit-code`. The public docs site is an Oxide app (`examples/docs`, content collections plus `search`, F09), so the framework dogfoods its own pipeline. **Acceptance:** adding a config key without a schema description fails `docs-check`.

### V2-E12 Monorepo and workspace support (S, no dependencies)

Detect `pnpm-workspace.yaml` or `package.json` `workspaces`. Resolve linked packages with `oxc_resolver` and `preserveSymlinks: false` so edits to `../packages/ui/src` are watched in dev (add those real paths to the watcher roots) and HMR boundaries cross package edges; the CAS key uses package name plus content hash instead of absolute paths so a moved checkout still hits cache. `oxide build --filter <pkg>` builds one workspace app; per-app `oxide.config.ts` may `extends: '../../oxide.config.base.ts'`. **Tests:** a fixture workspace with `apps/web` and `packages/ui`: editing `ui` triggers HMR in dev; two checkouts in different directories produce equal `dist-digest`. **Acceptance:** both tests pass on Linux and macOS.

### V2-E13 Svelte streaming SSR (S, spike)

The Svelte adapter falls back to `renderToString` (V2.1.1). Spike: does the pinned Svelte 5 release expose async or streaming server rendering (check the release notes and `svelte/server` exports for the current version)? If yes, implement `renderStream` in `packages/svelte/src/real-adapter.ts`, flip `capabilities.suspense` to `'native'`, and pass the existing `defer-swap` conformance assertion; if not, keep `suspense: 'core'` and record the version checked in `docs/decisions/E13-svelte-streaming.md`. **Kill:** the API is flagged experimental and unstable for the pinned version.

## V2.10 Wave R: release

### V2-R02 Capability matrix v2 (S, do this in week 0)

**Goal.** Add capabilities without ever editing existing rows (D39), keep the v0.1 rows byte-identical, and make every build refuse configurations a target cannot honor.

**Evidence.** `capability_matrix` (`crates/adapters/src/lib.rs:88`) builds rows with the inner `fn row(pairs: [(Capability, FeatureSupport); 7])`, so adding an eighth capability is a compile error in every arm.

**Change.** Replace the table by a function of `(target class, capability)`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    // v0.1 (keep order: BTreeMap iteration order is observable in docs and doctor output)
    L0Cache, Isr, Sockets, ImageOptimization, NativeRules, Snapshots, Tls,
    // v0.2
    MixedIslands, Live, Resume, Skew, DeltaDeploy, RustFns, Search, Offline, Brownout, Permissions, Attestation,
}

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::L0Cache, Capability::Isr, Capability::Sockets, Capability::ImageOptimization, Capability::NativeRules, Capability::Snapshots, Capability::Tls,
        Capability::MixedIslands, Capability::Live, Capability::Resume, Capability::Skew, Capability::DeltaDeploy, Capability::RustFns, Capability::Search,
        Capability::Offline, Capability::Brownout, Capability::Permissions, Capability::Attestation,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class { Native, NodeLike, Static, Edge, Serverless }

fn class(t: Target) -> Class {
    match t {
        Target::Binary | Target::Docker => Class::Native,
        Target::Node | Target::Deno | Target::Bun => Class::NodeLike,
        Target::Static => Class::Static,
        Target::Cloudflare | Target::CloudflarePages => Class::Edge,
        Target::Lambda | Target::Vercel | Target::VercelFunctions | Target::Netlify => Class::Serverless,
    }
}

pub fn support(t: Target, c: Capability) -> FeatureSupport {
    use Capability::*; use Class::*; use FeatureSupport::{Degraded as D, Full as F, Unsupported as U};
    match (class(t), c) {
        // v0.1 rows, copied from the old table cell by cell (a golden test pins them)
        (Native, L0Cache | Isr | Sockets | ImageOptimization | NativeRules | Snapshots | Tls) => F,
        (NodeLike, Snapshots) => D, (NodeLike, L0Cache | Isr | Sockets | ImageOptimization | NativeRules | Tls) => F,
        (Static, L0Cache | ImageOptimization | NativeRules) => D, (Static, Isr | Sockets | Snapshots | Tls) => U,
        (Edge, L0Cache | Isr) => D, (Edge, Sockets | ImageOptimization | Snapshots) => U, (Edge, NativeRules | Tls) => F,
        (Serverless, L0Cache | Isr | ImageOptimization | Snapshots) => D, (Serverless, Sockets | Tls) => U, (Serverless, NativeRules) => F,
        // v0.2
        (Native | NodeLike | Static | Serverless, MixedIslands) => F, (Edge, MixedIslands) => D,
        (Native | NodeLike, Live) => F, (Serverless, Live) => D, (Static | Edge, Live) => U,
        (Native | NodeLike, Resume) => F, (Static | Edge | Serverless, Resume) => U,
        (Native, Skew) => F, (NodeLike | Static | Edge | Serverless, Skew) => D,
        (Native | Static, DeltaDeploy) => F, (NodeLike | Edge | Serverless, DeltaDeploy) => D,
        (Native, RustFns) => F, (NodeLike | Static | Edge | Serverless, RustFns) => U,
        (Native, Search) => F, (NodeLike | Static | Serverless, Search) => D, (Edge, Search) => U,
        (_, Offline) => F,
        (Native, Brownout) => F, (NodeLike, Brownout) => D, (Static | Edge | Serverless, Brownout) => U,
        (Native, Permissions) => F, (NodeLike, Permissions) => D, (Static | Edge | Serverless, Permissions) => U,   // Native is Full only if F19 passes; otherwise change to U
        (_, Attestation) => F,
    }
}

pub fn capability_matrix(target: Target) -> BTreeMap<Capability, FeatureSupport> {
    Capability::ALL.iter().map(|c| (*c, support(target, *c))).collect()
}
```

The match is exhaustive over `(Class, Capability)` pairs, so the compiler proves every new capability has a value for every target class; adding a capability means adding arms, never touching others.

**Refusal rule (extends the existing `degradation`/refuse check).** `check_config(cfg: &Config, t: Target) -> Vec<Diagnostic>`: for each enabled feature, if `support(t, cap) == Unsupported` return an error diagnostic with the code from the registry (V2.K): `live` gives `AST-LIV-001`, `rust functions present` gives `RFN-002`, `fonts.subset: 'used'` with dynamic routes gives `AST-FNT-001`, `search` on edge gives `AST-SRC-001`; `Degraded` yields a warning with the documented degradation string (extend `degradation()` with the new pairs). The build prints the matrix row for the chosen target at the end of `oxide build --explain`.

**Tests (red first).** `v01_rows_unchanged`: a golden array of the 5 x 7 old cells compared with `support`; `every_target_every_capability` (iterate `Target::ALL` if present, else the enum list); `refuses_live_on_static`; `warns_degraded_search_on_node`; `doc_table_matches_matrix` (E11 generated table equals the function output). **Acceptance:** `cargo test -p oxide-adapters` green; `oxide doctor` prints a column per capability.

### V2-R01 Conformance v2 (M, after each F WP)

**Rule.** One assertion per user-visible feature, run against all four adapters on the Atlas reference app (`conformance/`). The harness reads the matrix: an assertion whose capability is `Unsupported` for the tested target is **skipped with a recorded reason**; a capability marked `Full` that fails is a release blocker (the matrix must tell the truth). New assertions (id, check, adapters):

| ID | Assertion | Adapters |
| --- | --- | --- |
| C25 | HTML bootstrap contains `build:` and `/__data` carries `x-oxide-build` (A04) | all 4 |
| C26 | Stale build header gets 409 with `x-oxide-build-stale` (A04) | all 4 |
| C27 | Parent-dependent and independent loads overlap (timing, A06) | all 4 |
| C28 | Ancestor middleware chain runs outer to inner and locals merge (A05) | all 4 |
| C29 | Gated route's `/__data` is gated too (A05) | all 4 |
| C30 | Prerendered error page is served with its error status (A07) | all 4 |
| C31 | Four-framework mixed islands hydrate independently (F01) | react host page |
| C32 | Pre-hydration click is replayed exactly once (F03) | all 4 |
| C33 | Experiment variants get separate L0 entries; no-experiment routes keep the old key (F04) | all 4 |
| C34 | Why-dynamic reports line and snippet; `--fix` flips mode (F16) | all 4 |
| C35 | Live route updates after `revalidate` within 200 ms (F05) | all 4 |
| C36 | Patch payload applies; mismatched base falls back to full (F06) | all 4 |
| C37 | Resumable stream reattach yields byte-identical total (F07) | endpoint, all 4 |
| C38 | Brownout sheds low-priority and keeps critical routes (F12) | all 4 |
| C39 | Old build's server fn call succeeds inside the skew window (F10) | all 4 |
| C40 | Rust server function callable from the client (F08, binary only) | all 4 |
| C41 | Search returns ranked hits and works as static JSON (F09) | all 4 |
| C42 | Learned prefetch picks the dominant edge (F13) | all 4 |
| C43 | Offline submit yields exactly one server insert (F14) | all 4 |
| C44 | Record then replay is byte-identical (F15) | all 4 |
| C45 | Chunks carry `integrity` and match the manifest (E04) | all 4 |
| C46 | Typed route fixture compiles and rejects bad calls (E02) | all 4 |
| C47 | HMR keeps input state across a component edit (E01) | all 4 |

Implementation: each assertion is a function in `conformance/assertions/<id>.ts` returning `{ ok, detail }`, registered in `conformance/run.ts` with its `capability` and `adapters` fields; `node run.ts --adapter react --target binary` prints a table; CI runs the cross product `{4 adapters} x {binary, node}`. Browser-dependent assertions call into the E10 Playwright specs. **Acceptance:** 24 + 23 = 47 assertions per adapter; zero unexplained skips.

### V2-R03 Benchmark report and gate refresh (S, depends on the B-series)

Re-run `benchmarks/run.sh`, `coldstart.sh`, `hmr.sh`, `load.sh` and the criterion summary; rewrite `benchmarks/report.md` with the commit hash, hardware, tool versions and the V2.2.3 target table filled in with before and after numbers; keep misses visible with a plan. Competitor comparison (Next.js, Nuxt, SvelteKit, SolidStart, Vite) is **re-measured at release time with then-current versions** (never carried over from older reports). `gate.sh` tracks the new load metrics; the criterion comparison runs in CI. **Acceptance:** `gate.sh` passes against the new report and fails against a deliberately regressed build.

### V2-R04 Docs, README and architecture refresh (S, depends on all)

README: current status, crate and package counts, honest limitations list (what is `Degraded` where, what is spike-only), quickstart for each adapter; `docs/architecture.md` regenerated (A03) plus a new section per v0.2 subsystem (generations, live hub, stream store, governor, rust fn host, search); `CHANGELOG.md` for v0.2; migration notes (below). Every config key and error code documented via E11 generation.

### V2-R05 Security review checklist (M, depends on F05, F07, F08, F09, F13)

| Surface | Threat | Required control and test |
| --- | --- | --- |
| `/__oxide/live` | Cookie replay, subscriber exhaustion, cross-origin read | Origin and `Sec-Fetch-Site` check; per-IP cap; max subscribers; cookie snapshot dropped on close; test with forged Origin |
| `/__oxide/stream/:id` | Guessing ids, reading another user's stream | 128-bit random ids; bind id to the original request's session cookie hash; mismatch gives 410; test |
| `/_oxide/fn/*` (incl. Rust) | CSRF, arbitrary export call | POST only, origin, build header, stable-hash ids with secret, input schema validation; ABI version check |
| Rust function host | Memory unsafety across FFI, panics | `catch_unwind`, C ABI only, bounded buffers, timeouts, library copied before load; fuzz the shim decoder |
| `/_oxide/search` | Query DoS, regex injection | Query length cap (200), result cap (50), fuzzy only on long terms, rate limit |
| `/_oxide/nav` | Privacy leakage | k-anonymity threshold, route ids only, no identifiers; test sparse edges are hidden |
| Experiments | Cache poisoning across variants | Variant in the cache key; `set-cookie` never stored; test two buckets |
| Record and replay | Secret leakage | Default redaction of `authorization`, `cookie`, `set-cookie`; records are dev-only unless sampled with redaction forced on; `.oxide/records` in `.gitignore` template |
| ODP1 decoder | Prototype pollution, huge paths | Reject `__proto__` and `constructor`, depth and op count limits (64 ops), fuzz target `odf_patch` |
| Idempotency store | Replay of someone else's response | Key bound to session hash plus route; 24 h TTL; test cross-session miss |
| Generations | Old code with newly found vulns lingers | Window defaults to 15 minutes, drop-early on brownout, documented |
| Open-in-editor, cost, inspector | Local file access | Dev-token guard, project-root confinement, disabled in production builds |

Output: a signed-off table in `docs/security/v0.2-review.md` with each row's test name, plus fuzz targets added to `fuzz/` (`odf_patch`, `rustfn_shim`, `stream_range`).

### V2-R06 Release checklist (S, depends on all)

1. V2.2.4 gate items all true. 2. Version bump to `0.2.0` across crates and packages (`cargo set-version --workspace`, package.json versions, adapters' `contractMinor`). 3. **Migration notes:** run `oxide cache clear` (L0 key width and ODP1); route manifest archive version bump means a rebuild is required; `framework.compilerBackend` defaults to `auto` (native compilers change cold-build output hashes, not behavior); config keys added are all optional. 4. Tag, publish packages, attach binaries with `dist/attest` (F20) for the release itself. 5. Announce the capability table and the spike decisions.

## V2.11 Risks and open questions

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Native compiler crates are pre-1.0 or git-only; APIs and pins differ from the reports | C02 to C04 slip or are killed | Spike first with kill criteria; Node pool stays the default fallback; vendored, pinned sources |
| Multiple oxc versions in one binary | Binary size, compile time | `cargo tree -d` budget; string-only boundary; keep native features off in dev builds |
| `build_dispatch` refactor changes behavior | Regressions in a hot path | Digest equality, the existing suite, one stage per commit, `oxide-dispatch` unit tests with fakes |
| Router candidate index misses a candidate | Wrong route served | Superset test plus 200k-case nightly property test against the linear oracle; fuzz target |
| Mixed islands: four runtimes on one page | Heavy pages, hydration conflicts | Strategy-gated, per-framework vendor chunks, fixture to find incompatibilities, capability honesty |
| SSE behind proxies that buffer | Live updates delayed | `x-accel-buffering: no`, keepalive pings, poll transport fallback |
| Generation retention doubles memory | OOM on small hosts | Window config, brownout drops previous early, doc the cost |
| Rust function ABI misuse | Crashes in the server process | C ABI over bytes, `catch_unwind`, version check, timeouts; Binary and Docker only |
| Spike outcomes (F02, F19, C-series) unfavorable | Scope shrinks | Written decision records; spikes cannot block the release gate (they may close as `killed`) |
| Scope vs calendar | Late release | Order in V2.4.2; cut order: F17, F18, F13, F14, E11, E12 before touching Waves A to C or R |

**Open questions (decide in the first week and record in `docs/decisions/`).** 1. Final product name (Part I §26.1) before publishing packages. 2. Is the Cloudflare edge target a v0.2 gate or best effort (E09)? 3. `Permissions` row for Native: Full only if F19 passes; decide at the spike. 4. Whether to ship `native-*` compilers enabled by default in release binaries or opt-in for one release. 5. Redis or S3 as the first remote cache for CI (A10 implements both; documentation prioritizes one). 6. Whether `ODP1` patches are also used for `/__data` navigation responses (not only live), which needs a per-tab base store.

## Appendix V2.J Endpoint map

Plane rule (existing): `/__oxide/*` is the **data plane** (build-header checked, A04); `/_oxide/*` is the **ops plane**; dev-only surfaces are dev-token guarded (D41).

| Path | Method | Plane | WP | Purpose |
| --- | --- | --- | --- | --- |
| `/__data?route=` | GET | data | existing, A04, A05 | Route loader data as ODF; now gated by middleware and carries `x-oxide-build` |
| `/__oxide/live` | GET (SSE) | data | F05 | Live loader snapshots and patches |
| `/__oxide/stream/<id>` | GET | data | F07 | Reattach to a resumable stream with `Range` |
| `/__oxide/state` | GET | data | existing | Hydration state for tooling |
| `/_oxide/fn/<id>` | POST | ops | existing, F08 | Server functions (TypeScript and Rust) |
| `/_oxide/search` | GET | ops | F09 | Full-text query |
| `/_oxide/nav?from=` | GET | ops | F13 | Aggregate next-route probabilities |
| `/_oxide/csp-report` | POST | ops | E04 | CSP violation reports |
| `/_oxide/health`, `/_oxide/ready` | GET | ops | existing, F12 | Liveness and readiness (level reported) |
| `/_oxide/{vitals,static,og,metrics,image,hmr}` | various | ops | existing | Unchanged |
| `/__oxide/open`, `/__oxide/cost` | GET | dev | E08, F17 | Open in editor; component cost (dev-token guarded) |
| `/__oxide` | GET | dev | existing, E08 | Inspector |

A revalidation entry point already exists (locate with `rg -n "revalidate" crates/server/src crates/cli/src`); F05 hooks it and adds no new path.

## Appendix V2.K Error code registry additions

Keep the registry as `docs/errors.toml` (E11 generates `errors.md` from it and a test greps call sites against it). Entry format: `[CODE] message = '...'; fix = '...'`.

| Code | Meaning | Fix text |
| --- | --- | --- |
| `CMP-NAT-001` | `compilerBackend: 'native'` but no healthy native compiler for the framework | Build with `--features native-<fw>` or set `compilerBackend` to `auto` or `node` |
| `DSP-101` | A middleware set a reserved locals key (`session`, `locale`, `experiments`) | Rename the key; reserved keys are owned by the core |
| `DSP-110` | Invalid experiment definition (traffic outside 0 to 100, fewer than 2 variants) | Fix the `experiments` entry |
| `AST-LIV-001` | `live: true` on a target without long-lived connections | Use `live.transport: 'poll'` or change target |
| `AST-FNT-001` | `fonts.subset: 'used'` with non-static routes | Use `latin` or `unicode-range`, or make all routes static |
| `AST-SRC-001` | `search` configured for a target without the capability | Use static JSON search or a Binary or Docker target |
| `RFN-001` | Rust function library ABI version mismatch | Rebuild the user crate with the matching `oxide-rustfn` version |
| `RFN-002` | `*.server.rs` present but the target cannot load native functions | Use Binary or Docker, or remove the Rust functions |
| `RPL-002` | Replay saw an outbound call that is not in the record | Re-record, or the code path changed |
| `RPL-003` | Record was made on an older build id | Replay continues; results may differ |
| `OX-DYN` | Route is dynamic; reasons attached (F16) | Run `oxide why <route> --fix` |
| `OX-SRC-001` | Search params failed `searchSchema` validation | Fix the URL or add defaults to the schema |
| `CAC-REDIS`, `CAC-ZSTD`, `CAC-JSON` | Remote cache handler I/O, compression, serialization failures | Check connectivity and credentials; see message |
| `BLD-409` | Build header unknown or expired (surfaced client-side as a reload) | Reload the page |
| `PRM-001` | A route used a capability outside its declared permissions (F19) | Add it to `config.permissions` or remove the call |

## Appendix V2.L Configuration additions

All keys are optional; each needs a JSON Schema entry in `crates/config/src/schema.rs`, a typo-suggestion test, a docs line (E11) and an `OXIDE_*` env override where noted.

```ts
interface V02Config {
  framework?: { compilerBackend?: 'auto' | 'native' | 'node' }                    // C01 (env OXIDE_COMPILER_BACKEND)
  islands?: Record<string, string | { src: string; framework?: 'react' | 'vue' | 'svelte' | 'solid' }>   // F01
  interactionReplay?: boolean                                                       // F03, default true
  experiments?: Array<{ id: string; traffic: number; sticky: 'cookie' | 'header' | 'ip-hash'; cookieName?: string; headerName?: string; variants: string[]; routes?: string[] }>   // F04
  live?: { maxSubscribers?: number; debounceMs?: number; keepaliveSeconds?: number; maxRerunsPerSecond?: number; transport?: 'sse' | 'poll' }   // F05
  stream?: { resume?: { ttl?: string; maxBytesEach?: string; maxBytesTotal?: string } }   // F07
  brownout?: { enabled?: boolean; staleCeiling?: string; thresholds?: { queueWaitMs?: [number, number, number]; loopLagMs?: [number, number, number]; rssMb?: [number, number, number] }; hedge?: { enabled?: boolean; afterFactor?: number } }   // F12
  deploy?: { skewWindow?: string; generations?: number }                            // F10, F11 (defaults '15m', 3)
  prefetch?: { learned?: boolean; minProbability?: number }                         // F13
  offline?: { enabled?: boolean; precache?: 'routes' | 'all'; outbox?: boolean }    // F14
  search?: { collections: string[]; fields?: { title?: number; body?: number }; snippet?: number; languages?: string[] }   // F09
  record?: false | 'errors' | 'all' | { sample?: number; onError?: boolean }         // F15
  permissions?: { default?: 'allow' | 'deny'; net?: string[]; env?: string[]; fs?: string[] }   // F19
  security?: { sri?: boolean; trustedTypes?: 'off' | 'report' | 'enforce'; csp?: 'strict' | 'report-only' | 'off' }   // E04
  check?: { typecheck?: 'tsgo' | 'oxlint' | 'tsrs' | 'tsc' | 'off' }                // C11
  assets?: { svg?: { optimize?: boolean } }                                         // C07
  fonts?: { subset?: string[] | 'used' }                                            // C08
  images?: { avifSpeed?: number }                                                   // C09
  cache?: { handler?: { type: 'redis'; url: string; prefix?: string } | { type: 's3'; bucket: string; prefix?: string; region?: string; endpoint?: string } }   // A10
  routes?: { compat?: 'next' }                                                      // E03
  llms?: { enabled?: boolean; title?: string; summary?: string; include?: string[] } // E07
  workspace?: { filter?: string }                                                   // E12
}
```

Per-route options (`export const config` in `page.server.ts`) gain `live`, `priority: 'critical' | 'normal' | 'low'` (F12), `permissions` (F19), `resumable` is a response helper, not config.

## Appendix V2.M Environment variables

| Variable | Used by | Meaning |
| --- | --- | --- |
| `OXIDE_COMPILER_BACKEND` | C01 | Override `framework.compilerBackend` |
| `OXIDE_CACHE_DIR` | F18, CI | Shared CAS directory across worktrees or checkouts |
| `OXIDE_TEST_REDIS_URL`, `OXIDE_TEST_S3_ENDPOINT` | A10 | Enable remote-handler contract tests (otherwise printed as SKIP) |
| `OXIDE_REPLAY` | F15 | Record file the worker replays from |
| `OXIDE_EDITOR` | E08 | Editor command for open-in-editor |
| `OXIDE_SIGNING_KEY`, `OXIDE_BUILDER_ID` | F20 | Attestation signing key path and builder id |
| `OXIDE_CHECK_STRICT` | C11 | Missing lint or typecheck tool fails instead of SKIPPED |
| `SOURCE_DATE_EPOCH` | F20 | Fixed time for any reproducible timestamp |
| `OXIDE_BENCH_STRICT` | B09 | Gate fails on any regression instead of only above threshold |

## Appendix V2.N Agent runbook

### V2.N.1 The loop for every WP

1. `git switch -c v2/<wp-id>`; read the WP and its dependencies (V2.4); confirm dependencies are merged. 2. Record baseline numbers if the WP is B-series. 3. Write the failing tests named in the WP; run `cargo test -p <crate> <name>` and see them fail for the right reason. 4. Implement the smallest change. 5. `cargo fmt --all`, `cargo clippy -p <crate> -- -D warnings`. 6. Run the WP's *Observable proof* command and paste the output into `WORKLOG.md` under a heading `V2-<id>`. 7. Run the full gates (V2.0 item 8). 8. One commit per file with the WP id in the body; open a PR using the template below.

```
Title: V2-<id>: <short goal>
Evidence: <the symbol or line that proved the problem>
Change: <files touched, one line each>
Proof: <command and its output>
Numbers (B-series): <before> -> <after>
Matrix/conformance/schema/docs: <done | n/a because ...>
Risks and rollback: <one line>
```

### V2.N.2 Fast feedback

`cargo test -p oxide-router`, `-p oxide-cache`, `-p oxide-odf`, `-p oxide-dispatch`, `-p oxide-compiler` finish in seconds and are where new logic is tested. The V8-linked `oxide-cli` and `oxide-jshost` test targets are slow (minutes): run them once per WP, not per edit. Update golden or snapshot files only with the framework's flag (`INSTA_UPDATE=always` for insta) and review every diff; never regenerate goldens to make a failing test pass without understanding the diff. Property tests: on failure keep the printed seed in a regression test.

### V2.N.3 Failure triage

| Symptom | Likely cause | Action |
| --- | --- | --- |
| `Database already open` in tests | Concurrent test binaries share the redb CAS | Re-run once; use a temp dir per test for new tests |
| `cargo check` ambiguity errors after A01 | Two re-exported modules define the same name | Rename the later item; never `pub use` two same-named items |
| Dist digest changed after a pure refactor | Iteration order of a `HashMap` leaked into output | Use `BTreeMap` or sort before emitting |
| Router tests pass but fuzz target crashes | Candidate index assumption violated | Add the input as a regression case in the oracle test |
| Hydration mismatch after native compiler | Scoped-style ids or hash suffixes differ between SSR and client builds | Compare with `scripts/compiler-equiv.sh`; fix id derivation or keep Node for that framework |
| macOS command hangs | No `timeout` binary | Use the background PID plus `sleep N; kill` pattern |
| Server still running after test | Leaked child process | Kill by PID you started; never `pkill -f oxide-cli` |
| Tests that need `--ignored` skipped in CI | Engine-dependent tests | CI job `ignored` runs them on Linux; run locally before merging V8 changes |

### V2.N.4 Stop and ask conditions

Stop and write a note in `WORKLOG.md` (and ask the owner) when: a premise in a WP is false in the tree (for example a function named in the evidence no longer exists); a change would alter a public Part I contract (adapter contract, ODF1 bytes, manifest formats) beyond what the WP lists; a spike hits its kill criterion; or two WPs need conflicting edits to one file after A01.

### V2.N.5 Definition of done (all WPs)

Tests written first and green; observable proof pasted; gates green; no new `unwrap()` on request paths; no new global mutable state without a lock rationale in a comment; metrics and tracing spans added where the WP changes a pipeline stage; capability row, config schema, error codes and docs updated per V2.0 item 12; `WORKLOG.md` entry with commit hashes.

## Appendix V2.O Work package status table

Copy into `WORKLOG.md` and update as WPs merge. Status values: `todo`, `doing`, `merged`, `spiked-keep`, `spiked-killed`.

| WP | Status | PR | Notes |
| --- | --- | --- | --- |
| B09 | todo |  | baseline recorded in `benchmarks/baseline-v01.json` |
| A03 | todo |  |  |
| R02 | todo |  |  |
| A01 | todo |  |  |
| A02 | todo |  |  |
| B01 to B08 | todo |  |  |
| A04 to A10 | todo |  |  |
| C01 to C11 | todo |  |  |
| F01 to F20 | todo |  |  |
| E01 to E13 | todo |  |  |
| R01, R03 to R06 | todo |  |  |

## Appendix V2.P First day for an implementing agent

```bash
# 1. orient
cat CONVENTIONS.md HANDOFF.md docs/oxide-v0.2-feature-designs.md
git log --oneline -5            # expect 3975f2a at the top for this spec
# 2. prove the baseline is green before changing anything
cargo test --workspace --no-fail-fast 2>&1 | tail -20
pnpm test:packages && (cd conformance && node run.ts) && tests/fixtures/check.sh
# 3. record the four dist digests (A01 step 0) and the criterion baseline (B09)
for a in examples/smoke-app-*; do ./target/debug/oxide-cli build --root "$a" && scripts/dist-digest.sh "$a"; done
cargo bench --workspace && python3 benchmarks/criterion_summary.py benchmarks/baseline-v01.json
# 4. start Week 0: B09, A03, R02; then A01 per V2.4.2
```

If any command in step 2 fails on the unmodified tree, stop and fix or document that first: every later WP depends on a trustworthy baseline.
