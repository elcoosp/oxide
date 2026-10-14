# Oxide Implementation Worklog

Shared log for all agents. Read before working. Append after finishing a Task ID.

Project: Oxide meta-framework per `upload/Oxide Technical Specification v0.1.md` (spec §refs below use its numbering).
Repo root: `/home/z/my-project/oxide/` (git). Deliverable: zip in `/home/z/my-project/download/`.

---
Task ID: 1
Agent: main (Super Z)
Task: Wave 0 — workspace skeleton, pinned dependencies, conventions, crate stubs with cross-crate API contracts.

Work Log:
- Read full spec (781 lines). Extracted crate list (§23), decisions D1–D20, adapter contract (§7), ODF (§9.5), router (§8), cache (§11), server pipeline (§12), dev/HMR (§13), config (§14), plugins (§15), platform (§16), security (§17), deploy (§18), CLI (§19), errors (§19.4), conformance (§22).
- Installed Rust 1.99.0 (rustup, minimal profile). Verified crates.io + npm registry reachable. Node v24.21.0 available for NodeHost + TS tests.
- Created workspace skeleton: 16 spec crates + diagnostics crate, all compiling with stub APIs.
- Pinned all shared dependency versions in root [workspace.dependencies].

Stage Summary:
- Skeleton green: `cargo build --workspace` passes.
- Agents: work in your assigned crates ONLY. Never edit other crates' sources. Use `cargo test -p <pkg>`; a shared target dir means builds may wait on the lock (use timeout ≥ 300s). Read `/home/z/my-project/oxide/CONVENTIONS.md` first. TDD: write failing tests, then implement, then green.

---
Task ID: 2
Agent: odf (initial attempt died at context deadline; finished by odf2)
Task: crates odf + diagnostics — ODF wire format (§9.5) with golden fixtures.

Work Log (odf2, resumed):
- Fixed Result alias import collision; shared-container dedup slot bug (Ref(1) now emitted for repeated structures); ODF-MAGIC vs ODF-TRUNC ordering; refs depth guard at leaf level; integral f64 → JSON integer mapping.
- 16 golden fixture pairs written to crates/odf/tests/fixtures (+ examples/odf_fixture.rs regenerator, byte-stability test).

Stage Summary:
- 24 tests green; zero todo!; committed 8960a8b. Format doc maintained in decoder header + conformance/fixtures/README.md.

---
Task ID: 3
Agent: router (died at deadline; finished by router2)
Task: crates router — radix matcher, rkyv archive, typegen, rules engine (§8, D8, D13).

Work Log (router2):
- Fixed ferr helpers, ArchivedTree Debug, route-id canonicalization (`routes/` prefix), layout_chain derivation from route ids, mmap liveness test, shadow-DTO rkyv archive.
- 55 tests green (incl. 2 proptests); committed d9e7bc5.

Stage Summary:
- Scanner/matcher/typegen/rules all green; 6 documented minimal test adjustments (contract contradictions in pre-written tests).

---
Task ID: 4
Agent: graphcfg (died at deadline; finished by graphcfg2)
Task: crates graph + config — demand engine w/ early cutoff, CAS; layered typed config (§5, D6, §14, D14).

Work Log (graphcfg2):
- Epoch/early-cutoff semantics settled (seeds always reported; hash-stable dependents survive; dep-free nodes recompute after bump). Tool-version tracking added.
- Config: 6-source layering, OXIDE_ allow-list, Levenshtein typo suggestions, §14.1 example extraction test, JSON fallback flag.

Stage Summary:
- graph 35 tests, config 29 tests, all green; committed acd2327.

---
Task ID: 5
Agent: cache
Task: crate cache — L0/L1/L2, tags, SWR, single-flight (§11).

Work Log:
- In-house byte-weighted LRU (moka swap documented), redb disk tier w/ hash-verified blobs, single-flight publish-under-shard-lock design, soft/hard tag invalidation, conditional-request helpers.

Stage Summary:
- 44 tests green; committed 7e9e5dd; deviations logged in docs/deviations.md.

---
Task ID: 6
Agent: compbundle (died at deadline; compiler finished, bundler finished by bundler2)
Task: crates compiler + bundler — passes, guard, ESM bundler, manifests (§6.2–6.4, §9.3–9.4).

Work Log:
- compiler: lexer + 29 tests green (import scan, use_cache, server-fn stubs, guard, HMR inject).
- bundler (bundler2): resolve reuse + normalize() fix, shared-chunk grouping, export_names fix, real import rewriting, manifests roundtrip; 21 tests green.

Stage Summary:
- compiler 32 + bundler 21 tests green; commits cc67005 (+ compiler files synced at 2ae8772).

---
Task ID: 7
Agent: jshost
Task: crates jshost + plugin — JsHost trait, NodeHost child workers, ops, hooks (§4, §15).

Work Log:
- frame/unframe, choose_worker, ScriptedHost/ScriptedPool, NodeHost with 77-line pure-Node worker (render/compile/boom/die/sleep/chunked), V8Host fail-fast stub, plugin registry/chain/WASM validation.

Stage Summary:
- jshost 18 + plugin 11 tests green (7 real Node-process tests); committed e7a64de.

---
Task ID: 8
Agent: rendserv (died at deadline; server finished by servfix)
Task: crates render + server — streaming pipeline, boundary protocol, hyper pipeline, security (§7.1, §10, §12, §17).

Work Log:
- render: head merge, swapper/boundary protocol, islands, hydration bootstrap, lol_html inject (static + streaming byte-equal), 39 tests green.
- server (servfix): Send-future fix (sync native_prefix + BoxFuture crossing await), 48-test suite green incl. hyper integration, ws codec, security tables.

Stage Summary:
- render 39 + server 49 tests green; committed 6f422b9.

---
Task ID: 9
Agent: assetsdev
Task: crates assets + dev — CSS/image/font/content pipelines, watcher, HMR (§6.6–6.7, §16, §13, §5.5).

Work Log:
- CSS parser/nesting/modules/minify/prefixes; image resize/encode/LQIP; font bundles; metadata/sitemap/robots/markdown/highlight; hash-confirmed watcher (touch-without-change = no events); HMR messages + hub + behavior table.

Stage Summary:
- assets 49 + dev 12 tests green; committed 4640c5c.

---
Task ID: 10
Agent: cliadapt
Task: crates adapters + cli + core-wasm — deploy targets, capability matrix, CLI surface, edge subset (§18, §19.1, D18).

Work Log:
- Capability matrix + refusal rule + 10-target emit; all §19.1 commands incl. run_with_writer; core-wasm edge helpers.

Stage Summary:
- adapters 20 + cli 25 + core-wasm 12 tests green; committed e287710.

---
Task ID: 11
Agent: tsruntime
Task: packages/oxide — server APIs, ODF JS decoder, client router, islands, enhance, test helper.

Work Log:
- ODF decoder reverse-engineered from Rust fixtures (16/16 byte-exact, refs resolve as live references); router/enhance/islands tested against hand-rolled fake DOM.

Stage Summary:
- 66 tests green across 7 files; committed b71a1ff.

---
Task ID: 12
Agent: adapters (died at deadline; create-oxide finished by main)
Task: framework adapters react/vue/svelte/solid, conformance suite, create-oxide (§7, §22, §19.2).

Work Log:
- Four adapters (IR-based SSR, streaming boundary protocol, hydration, capabilities per §7) — 14 tests each.
- Conformance suite: assertion set × 4 adapters green (§22 list). create-oxide finished by main (integrator): programmatic scaffolding, 4 frameworks × 4 shapes, ENOEMPTY refusal, 5 tests green.

Stage Summary:
- TS: 14×4 + 4 conformance + 5 = 65 tests green; commits 264e346, 2de90c4.

---
Task ID: 13
Agent: main (Super Z)
Task: Integration, verification, smoke run, docs.

Work Log:
- Full `cargo test --workspace`: 477 passed / 0 failed (re-verified post-clean with debug=0).
- TS total: 131 tests (66 + 4×14 + 5 + 4), all green.
- Release build of oxide-cli (3m19s); smoke run: doctor, routes scan (root-page `/index`→`/` mapping fixed in cli dispatch + retested), build (2 routes, deploy artifacts), start (GET / 200, /blog/abc 200, /_oxide/health ok, 404s correct).
- Disk-full incident handled (cleaned 8.5GB debug artifacts).
- Docs: README.md, docs/architecture.md (§→crate map), docs/deviations.md cross-crate summary, benchmarks/README.md, examples/smoke-app, CI workflow (3-OS matrix), MIT/Apache dual license.

Stage Summary:
- Everything green; repo at d4de01d (15 commits).

---
Task ID: 14
Agent: main (Super Z)
Task: Zip deliverable.

Work Log:
- `git archive` of HEAD + WORKLOG.md → /home/z/my-project/download/oxide.zip (585 KB, 303 files).

Stage Summary:
- Deliverable ready.

---

## Session — §6.3 / §7 framework adapter read path CLOSED

**Milestone:** the read-path gap that blocked "a team can ship a production
site using only Oxide" is now closed for all four frameworks.

What landed this session:

- `oxide-jshost::FrameworkHost` — Rust-side loader + render dispatcher for
  a user's `oxide.config.{ts,mjs,js}` (spec §6.3, §7). Talks to the Node
  worker via two new ops: `framework:load` (descriptor extraction) and
  `framework:render` (dispatch to `FrameworkAdapter.render` + drain stream).
- Worker `oxide-worker.mjs` extended with the two ops; dynamic `import()`s
  the config, evaluates it, calls into the exported adapter.
- Adapter packages all four migrate to latest upstreams:
  - React `19.3.0` (stable)
  - Vue `3.6.0-rc.11` (Vapor Mode + reactivity rewrite)
  - Svelte `5.57.2`
  - Solid `2.0.0-rc.14` + `@solidjs/web` (async-native reactivity; the
    web renderer moved out of `solid-js/web` into a dedicated package)
- `createRealAdapter({ ssr, componentFor, layoutFor })` exposed from every
  `@oxide/{react,vue,svelte,solid}` package. Callable default export
  (`framework: react({ ... })` in config) returns the IR adapter by
  default and dispatches to the real renderer when `componentFor` is
  supplied.
- CLI subcommands:
  - `oxide config [--json] [--root <path>]` — prints the adapter descriptor
  - `oxide render <route> [--root] [--url] [--param k=v] [--data <json>]`
    — dispatches a page render through the adapter, writes HTML to stdout
- E2E tests prove the full chain:
  - `crates/jshost/tests/framework_host_e2e.rs` — real config → real
    `react-dom/server` → HTML, through the Rust worker
  - `crates/cli/src/tests.rs::cli_render_dispatches_real_react_ssr` —
    same, but through the CLI's `oxide render` subcommand

Both E2E tests are `#[ignore]`d by default (they spawn Node). Run with
`cargo test -- --ignored`.

**What remains for full pipeline integration:**

- `cmd_dev` / `cmd_build` / `cmd_start` still use the neutral-IR renderer.
  Wiring them to `FrameworkHost` is a plumbing change, not architecture:
  the loader + dispatcher + adapter contract are all proven end-to-end.
- Content-hashed build output, streaming through the pool (currently the
  worker collects the whole stream into one HTML string), and per-request
  caching in the dev server are follow-ups.

---

## Session 2 — framework adapter pipeline wired end-to-end

**Milestone:** the §6.3 / §7 read path is now fully wired from `oxide dev`
through to the real framework renderer. When a project has an
`oxide.config.{ts,mjs,js}`, the dev server dispatches page renders through
the config's `FrameworkAdapter` (real `react-dom/server`,
`@vue/server-renderer`, `svelte/server`, or `@solidjs/web`).

What landed:

- **`oxide-server::PageRenderer`** — framework-agnostic trait the pipeline
  dispatches to. `PageRenderInput` mirrors `packages/oxide/src/types.ts`
  `RenderInput` minus the ODS blob. `Pipeline::with_page_renderer` chains;
  `Pipeline::default()` no-ops.
- **`oxide-jshost::FrameworkHost`** — Rust-side loader + render dispatcher
  over the Node worker. Two ops: `framework:load` (descriptor extraction)
  and `framework:render` (dispatch + stream drain). Five unit tests, one
  end-to-end test against a real `.mjs` config (`framework_host_e2e.rs`).
- **`oxide-jshost::FrameworkHostRenderer`** — eager-load wrapper: loads the
  descriptor once at construction, then dispatches per request. Six unit
  tests + the two E2E tests above.
- **`oxide-cli::framework_shim::AdapterPageRenderer`** — bridges jshost's
  `FrameworkRenderer` to `oxide_server::PageRenderer`, converting the typed
  params map to JSON. Five unit tests with a fake inner renderer.
- **`oxide-cli::build_dispatch`** — extracted the inline dispatch closure
  from `start_dev_server_full` into a top-level fn that takes
  `Option<Arc<dyn PageRenderer>>` and branches: renderer present → dispatch
  via it; else IR placeholder.
- **`cmd_dev`** — now computes `page_renderer` from `oxide.config` before
  starting the server. Config present + loads → attached (banner line);
  fails → warning + IR fallback; absent → IR path.
- **Upstream migrations** (this session, earlier slices):
  - React `19.0.0 → 19.3.0` (stable)
  - Vue `3.5.x → 3.6.0-rc.11` (Vapor Mode + reactivity rewrite)
  - Svelte `5.x → 5.57.2`
  - Solid `1.9 → 2.0.0-rc.14` + `@solidjs/web` (async-native reactivity)

`oxide dev` now prints `framework renderer: react (/path/to/oxide.config.mjs)`
when the adapter is live, and `warn: framework renderer not wired: <reason>`
when it fails to load.

What remains for full coverage:

- **`cmd_build` / `cmd_start`** still pass `None` for `page_renderer`.
  Same pattern as `cmd_dev`, needs the same wiring plus a decision about
  caching the descriptor across requests.
- **The Node worker collects** the adapter's `ReadableStream<Uint8Array>`
  into one HTML string. Chunked streaming through the pool (so SSR flushes
  as it's produced) is a follow-up.
- **HMR refresh** doesn't yet notify the renderer; the dev server's watch
  loop restarts the pipeline, but the adapter stays loaded in the Node
  worker's module cache.
- **The `oxide render` subcommand** uses `FrameworkHost::render` directly;
  it could be refactored to route through the shim + `AdapterPageRenderer`
  for a single code path.

---

## Session 3 — all three HTML-producing CLI commands wired (§7 close)

**Milestone:** the framework-adapter read path now dispatches real
`react-dom/server` / `@vue/server-renderer` / `svelte/server` /
`@solidjs/web` HTML from **every CLI command that produces HTML**:

| Command        | Purpose                     | Renderer wiring     |
|----------------|-----------------------------|---------------------|
| `oxide dev`    | dev server SSR              | ✅ (session 2)      |
| `oxide serve`  | `oxide start`/`preview` SSR | ✅ (this session)   |
| `oxide build`  | build-time prerender        | ✅ (this session)   |

Every command follows the same pattern:

    find_config_file(root) → framework_shim::build_adapter_renderer
    → Option<Arc<dyn PageRenderer>>
    → dispatch (dev/serve) or write_prerender (build)

When the project has no `oxide.config.{ts,mjs,js}`, every command falls
back to the neutral-IR placeholder with a printed warning — dev stays
usable, builds stay reproducible.

**Code landed this session:**

- **`crates/server/src/page_renderer.rs`** — `PageRenderer` trait +
  `PageRenderInput`. Framework-agnostic; the pipeline holds it as
  `Option<Arc<dyn PageRenderer>>`.
- **`Pipeline::with_page_renderer`** builder + `page_renderer()` accessor.
  `Pipeline::default()` added (struct literals updated at 6 test sites +
  1 CLI site).
- **`crates/cli/src/framework_shim.rs`** — `AdapterPageRenderer` bridges
  `oxide_jshost::FrameworkRenderer` to `oxide_server::PageRenderer`,
  converting the params `BTreeMap<String, String>` to JSON. Five unit
  tests against a fake inner renderer.
- **`crates/cli/src/lib.rs::build_dispatch`** — extracted from the inline
  closure in `start_dev_server_full`; takes
  `Option<Arc<dyn PageRenderer>>` and branches: renderer present →
  dispatch via it with status 200 / 500 on error; else IR placeholder.
- **`cmd_dev` / `cmd_serve` / `cmd_build`** — each constructs the
  renderer via `framework_shim::build_adapter_renderer` and threads it
  through. `cmd_build` gained helper fns `url_path_from_rel` +
  `write_prerender` (async, replaces the previous sync `push_html`
  closure).

**Tests:**

- 3 dispatch tests (`dispatch_routes_through_page_renderer_when_attached`,
  `dispatch_uses_ir_placeholder_when_no_renderer`,
  `dispatch_propagates_renderer_error_as_500`) prove the branch logic
  without Node.
- 5 shim tests prove input conversion + error propagation.
- Existing jshost E2E (`framework_host_e2e.rs`, `#[ignore]`d) and CLI
  E2E (`cli_render_dispatches_real_react_ssr`, `#[ignore]`d) prove the
  full chain against real Node + real React.

**What remains for full data-layer integration:**

- Loader output reaching the renderer. Today `PageRenderInput::new`
  passes empty `data`; the pipeline needs to invoke the route's
  `page.server.ts` load function and thread the ODF-decoded result
  into `PageRenderInput.data`. This is the next milestone — the
  adapter contract has the slot, the pipeline just doesn't fill it yet.
- Streaming through the pool. The Node worker collects the adapter's
  `ReadableStream<Uint8Array>` into one HTML string. Chunked streaming
  is a follow-up.
- HMR refresh notifying the renderer. The dev watch loop restarts the
  pipeline but the adapter stays in the Node worker's module cache.

---

## Session 4 — framework-adapter read path complete + HMR wired

**Milestone reached:** the §6.3 / §7 framework adapter read path is
functionally complete end-to-end. Every HTML-producing CLI command
dispatches through the config's adapter when one is present, and dev
supports live file-change invalidation.

### What landed this session (L1–L6)

**Loader phase (spec §9.1)**
- `crates/server/src/loader.rs` — `LoaderRunner` trait + `LoaderRunInput`
  + `LoaderOutput` + `short_circuits()` predicate
- `crates/jshost/src/loader_host.rs` — `LoaderHost` dispatching the
  worker's `loader:run` op
- `crates/cli/src/loader_shim.rs` — `AdapterLoader` bridging the two
- Worker `loader:run`: imports the module, calls `load(event)`, converts
  `error(status)` / `redirect(status, location)` sentinels into
  `{ status, location }` ok-payloads (not worker errors)

**Framework stack (spec §7)**
- `crates/cli/src/framework_shim.rs::FrameworkStack` — bundles
  `page_renderer` + `loader_runner` + `pool` under one struct built once
  per CLI command
- All three commands (`cmd_dev`, `cmd_serve`, `cmd_build`) construct the
  stack from `oxide.config.{ts,mjs,js}` via `build_framework_stack`

**Dispatch wiring (spec §9.1 → §7)**
- `build_dispatch` runs a two-phase dispatch: loader first (when route
  has `has_server_module`), then renderer
- Non-200 loader output short-circuits the response; 200 threads data
  into `PageRenderInput.data`
- 3 fake-loader tests: data threading, 404 short-circuit, 303+Location
  short-circuit

**Wire protocol (spec §4.2)**
- Worker emits `framework:render` as chunked `call_chunk` frames (8 KB
  of JSON-wrapped HTML) + `call_end`, replacing the single `call_ok`
- Rust side unchanged: `Stream::collect` handles both frame shapes
- Future-proofs the wire for streaming adapters (React 19, Solid, Vue 3.6)

**HMR (spec §13.3)**
- Worker: `moduleGeneration` counter + `versioned()` URL tagger applied
  to every dynamic import; `framework:invalidate` op bumps the counter
- `FrameworkHost::invalidate()` + 3 unit tests
- `FrameworkStack.pool` kept alive so `cmd_dev` has a handle after the
  renderer/loader have been moved into the pipeline
- `cmd_dev` calls `invalidate_framework_modules` on every watcher event
  (both `--once` and the real dev loop)
- E2E test proves: import module v1 → rewrite file to v2 → reimport still
  v1 (cached) → invalidate → reimport picks up v2

**Test race fix**
- Loader + framework E2E `unique_nonce()` gained an `AtomicU64` counter.
  Prior form (`{pid}-{nanos}`) collided on macOS's ~1µs clock resolution
  when parallel `#[tokio::test]`s called it at the same instant,
  producing a shared filename where one test's Drop removed the file the
  other was about to import. Fixed and verified with 5 parallel runs.

### Test coverage added

| Area | Level | Tests |
|------|-------|-------|
| LoaderRunner trait | unit (server) | 6 |
| LoaderHost | unit (jshost, ScriptedHost) | 4 |
| AdapterLoader shim | unit (cli, pure conversion) | 6 |
| Loader dispatch | unit (cli, fake loader) | 3 |
| Loader E2E | E2E (jshost, real Node) | 5 |
| Chunked frames | E2E (existing framework_host_e2e now exercises) | 2 |
| `invalidate` method | unit (jshost) | 3 |
| HMR E2E | E2E (jshost, real Node) | 2 |

### What remains (none block "team can ship")

1. **`cmd_build` build-time loader evaluation** — static prerender still
   emits empty `data`. Wiring the loader into build is the same shape as
   dev/serve dispatch, but with build-time semantics (no HTTP request).
2. **Streaming consumers in the pipeline** — the wire format is ready;
   the pipeline still collects. A `FrameworkHost::render_streaming` that
   returns the uncollected `CallStream`, plus a pipeline path that writes
   chunks as they arrive.
3. **`V8Host` for `render_pool`** — the migration from `NodeHost` to an
   in-process `V8Host` for the hot render+load loop. Gated on
   `deno_node` integration for npm resolution. The `JsHost` / `Pool`
   abstraction is already in place; the swap is one line in
   `build_framework_stack`.

### Disk note

The machine hit 100% during the L6 test run. The workspace `target/`
directory is ~30 GB; the host has ~3 GB free after cleanup. Any cargo
link step will fail until the host is freed (Time Machine local
snapshots, Xcode DerivedData, Docker, brew cleanup). This is a
host-level constraint, not a code problem.

---

## Session 5 — framework adapter read path: complete

**Milestone:** `oxide dev`, `oxide serve`, `oxide build` all dispatch
page renders through the project's `oxide.config.{ts,mjs,js}` framework
adapter, end-to-end and warning-free, against a real project fixture.

### What landed this session (L1–L13)

**Loader phase (spec §9.1)**
- `oxide_server::LoaderRunner` trait + `LoaderRunInput` / `LoaderOutput`
- `oxide_jshost::LoaderHost` (worker `loader:run` op)
- `oxide_cli::loader_shim::AdapterLoader` bridging the two
- Loader phase runs inside `build_dispatch`, threads `data` to the
  renderer, short-circuits on `error(...)` / `redirect(...)`
- Real-loader E2E (`loader_host_e2e.rs`, 5 tests) exercises a real
  `.mjs` module through the worker

**Framework stack (spec §7)**
- `FrameworkStack` bundles `page_renderer` + `loader_runner` + `pool`;
  built once per CLI command via `build_framework_stack`
- All three HTML-producing commands (`dev` / `serve` / `build`)
  construct it from the config

**Wire protocol (spec §4.2)**
- Worker emits `framework:render` as chunked `call_chunk` frames
- `FrameworkHost::render_streaming` returns the raw `CallStream`
  without collecting (ready for a streaming HTTP body)

**HMR (spec §13.3)**
- Worker `moduleGeneration` counter + `versioned()` import tagger
- `framework:invalidate` op + `FrameworkHost::invalidate()`
- `cmd_dev` watcher invalidates on file change
- HMR E2E: load module v1, rewrite to v2, reimport still v1 (cached),
  invalidate, reimport sees v2

**TSX compilation**
- `crates/jshost/worker/tsx-loader.mjs` — sucrase-based Node ESM hook
- `crates/jshost/worker/register-hooks.mjs` — registers the loader via
  the Node 22 `--import register()` API (no `--experimental-loader`
  deprecation warning on worker startup)
- `NodePool::spawn_child` passes `--import <register-hooks.mjs>`
- The smoke-app's `oxide.config.ts` now imports its own `page.tsx`
  files; the adapter's `componentFor` resolves to real React components

**Path + route-id fixes**
- `cmd_dev` / `cmd_serve` / `cmd_build` canonicalize their root argument
  once at entry — eliminates a doubled-path class of bug in the bundler
  and route scanner
- `build_dispatch` and `write_prerender` normalize the router's file-role
  id (`routes/index/page`) to the adapter's directory key
  (`routes/index`) via `strip_suffix("/page")`
- The enriched `server.manifest.json` is now written back to disk; the
  adapters emit to `dist/deploy/`, so the in-memory enrichment was lost
  before

**Fixture**
- `examples/smoke-app` is a workspace member: real `.ts` config, real
  `.tsx` pages, real `page.server.ts` for the dynamic route
- `oxide render routes/index` → `<h1>Home</h1>`
- `oxide render routes/blog/[slug] --param slug=X` → `<h1>Post: X</h1>`
- `oxide dev` serves both over HTTP with real React SSR
- `oxide build` prerenders `/index` to `dist/prerendered/index.html`

### E2E coverage

| Suite | Command | Tests |
|-------|---------|-------|
| `smoke_app_renders_through_real_config` | CLI subcommands | 1 |
| `smoke_app_builds_its_own_pages` | oxide build | 1 |
| `dev_server_serves_smoke_app_over_http` | HTTP round-trip | 1 |
| `cli_render_dispatches_real_react_ssr` | oxide render | 1 |
| `framework_host_e2e` | real config → react-dom/server | 2 |
| `loader_host_e2e` | real loader via worker | 5 |
| `hmr_e2e` | invalidate → module reload | 2 |

### What remains

1. **Streaming consumers in the pipeline** — `FrameworkHost::render_streaming`
   returns the raw `CallStream`, but `PageRenderer::render` still returns
   `String` and the pipeline collects. True streaming needs a trait
   change on `PageRenderer` (chunks, not a whole HTML) plus a hyper body
   writer that consumes them. Wire format is already ready; this is a
   perf improvement (TTFB), not a capability gap.

2. **`V8Host` for `render_pool`** — the in-process V8 migration gated on
   `deno_node` integration for npm resolution. The `JsHost` / `Pool`
   abstractions are already in place; the swap is one line in
   `build_framework_stack` once the integration lands. Post-MLP per the
   spec's roadmap.

3. **`cmd_build` writes an error HTML file on loader non-200** — currently
   logs to the artifact rather than failing the build. A `--strict-build`
   flag could make this a hard failure at CI time.

### Disk note

The machine ran between 1.5 GB and 12 GB free across the session, with
frequent spikes to 100% during cargo link steps. Each heavy slice
required manual cleanup of `target/` artifacts. If work continues, the
host needs ~30 GB free before the next multi-crate change.

---

## Session 6 — framework adapter read path, streaming, and runtime generateParams

**Milestone:** every HTML-producing command (`config`, `render`, `build`,
`dev`, `serve`) now goes through the framework adapter end to end with
loaders, TSX, HMR, streaming, and runtime `generateParams`. All E2Es
green against the `examples/smoke-app` fixture.

### What landed this session (L14–L21b)

**Streaming trait chain** (spec §7, §4.2)
- `FrameworkRenderer::render_streaming` (jshost) with default collect
- `FrameworkHostRenderer::render_streaming` overrides to forward each
  worker `CallStream` chunk (no buffer)
- `PageRenderer::render_streaming` (server) with the same default
- `AdapterPageRenderer` (cli shim) forwards through
- `OxideResponse::stream(status, BodyStream)` — new body variant
- `to_hyper_response` handles both branches: streaming uses chunked
  transfer, collected sets content-length
- Body item error type moved from `Infallible` to `io::Error` so a
  worker that crashes mid-response aborts the connection

**Wire-level progress** (spec §4.2)
- Worker emits each adapter chunk as its own `call_chunk` frame (no
  concatenate + reslice)
- Raw HTML on the wire, not a JSON envelope
- `oxide render --stream` exercises it at the CLI

**Pipeline integration**
- `build_dispatch` calls `renderer.render_streaming()` when a renderer
  is attached; bridges the `Send`-only RenderStream to the `Send + Sync`
  BodyStream via a small mpsc pump helper
- `oxide dev` and `oxide serve` stream over HTTP; the E2E asserts
  `Transfer-Encoding: chunked` and the absence of `Content-Length`

**Runtime `generateParams`** (spec §10.5)
- Worker `generate-params:run` op imports the module, calls the export
  (function or value), normalizes to a flat string map
- `LoaderHost::generate_params` returns the enumerated entries
- `cmd_build`'s Runtime arm evaluates through the worker instead of
  printing "requires runtime evaluation (post-MLP)"

**TSX compilation** (spec §6.3)
- Worker registers `register-hooks.mjs` (calls `module.register()`) via
  `--import` — warning-free replacement for `--experimental-loader`
- `tsx-loader.mjs` transforms `.tsx`/`.jsx` via sucrase

**Path and route-id fixes**
- `cmd_dev`/`cmd_serve`/`cmd_build` canonicalize `root` at entry to
  prevent path doubling through the bundler and route scanner
- `build_dispatch` and `write_prerender` normalize the router's
  file-role id (`routes/index/page`) to the adapter's directory key
  (`routes/index`) via `strip_suffix("/page")`
- `cmd_render` uses `load_config_or_default` so a config the Rust
  extractor can't fully parse still renders (the worker re-loads it)

### Test coverage

| Suite | Tests |
|-------|-------|
| `smoke_app_renders_through_real_config` | 1 |
| `smoke_app_builds_its_own_pages` (with runtime generateParams) | 1 |
| `dev_server_serves_smoke_app_over_http` (chunked assert) | 1 |
| `start_serves_smoke_app_over_http` (production) | 1 |
| `cli_render_dispatches_real_react_ssr` | 1 |
| `framework_host_e2e` | 3 (incl. worker chunk preservation) |
| `loader_host_e2e` | 5 |
| `hmr_e2e` | 2 |
| Server streaming unit tests | 3 |
| CLI lib tests | 67 |
| jshost lib tests | 39 |
| server integration tests | 57 |

`cargo test --workspace` and the full TS suite green.

### What the smoke-app produces

`oxide build examples/smoke-app` now yields:

```
dist/prerendered/index.html            → <h1>Home</h1>
dist/prerendered/blog/hello-prod.html  → <h1>Hello hello-prod</h1>
dist/prerendered/blog/second-post.html → <h1>Hello second-post</h1>
```

Both routes classified `static` in the manifest. The blog route's
`generateParams` is a runtime-built array (`.map`), so its enumeration
was evaluated through the worker — the mechanism that used to print
"requires runtime evaluation (post-MLP)" is now live.

### What remains

1. **V8Host migration for `render_pool`** — post-MLP, gated on
   `deno_node` integration for npm resolution. One-line swap in
   `build_framework_stack`. The `JsHost` / `Pool` abstraction already
   supports it.
2. **Deployment adapters** (Lambda / Vercel / Netlify full artifacts) —
   orthogonal to the adapter path.
3. **Conformance suite integration** — the §22 suite could include the
   smoke-app fixture as an additional cross-check for the in-process
   assertions.
4. **Docs** — README could describe `examples/smoke-app` as the
   reference project for a new contributor.

None of these are blockers. The meta-framework the spec describes can
now build and ship a working production site.

## Session 7 — bundler transform gap CLOSED (handoff §3)

**Date:** 2026-10-13 (continuation)
**Slice:** `oxide build` serverless targets now emit valid JavaScript.

### The bug (confirmed by handoff §3)

`crates/bundler/src/lib.rs`'s `bundle_impl` read each module's source,
hashed it, scanned imports, resolved dependencies, then wrote the **raw
source** straight into the chunk body. It never invoked
`oxide_compiler::OxcTransformer`. Every serverless target (Lambda /
Vercel / Netlify) emitted a deploy artifact whose SSR chunks carried
`interface Props {` and raw `<div>` JSX, which `node --check` rejects:

```
$ node --check dist/server/routes-index-page-*.js
SyntaxError: Unexpected token '<'
```

`oxide dev` / `render` / `serve` were unaffected because they compile
through the worker's sucrase hook (`tsx-loader.mjs`) at module-load
time, against source on disk, not against emitted chunks.

### What landed this session

**Compiler (`crates/compiler`)** — a real lowering stage:

- `oxc_backend.rs` gains `lower_to_js`, called at the end of
  `OxcTransformer::transform`. Sequence: `oxc_parser::Parser` on the
  post-pass source → `oxc_semantic::SemanticBuilder::new().build()`
  for scoping → `oxc_transformer::Transformer::new` with
  `TypeScriptOptions::default()` + `JsxOptions::enable()` (automatic
  runtime → `react/jsx-runtime`) → `oxc_codegen::Codegen::new().build()`.
- Diagnostics from `lower_to_js` are pushed onto the existing
  `TransformOutput::diagnostics` vector; parse failure falls back to
  the unlowered code so the pipeline continues (with a diagnostic
  recording why).
- `Cargo.toml` gains `oxc_semantic`, `oxc_transformer`, `oxc_codegen`
  (all workspace-pinned to 0.153.0).
- Two `compiler_tdd` assertions on byte-exact quote style
  (`tags: ['a', "b", 'c']`) become semantic checks — codegen
  normalises `'` to `"` and re-flows arrays across multiple lines. The
  rewritten assertions verify the `tags:` option is present and each
  tag string is in the output.

**Bundler (`crates/bundler`)** — the wiring:

- `lib.rs` imports `Env as CompilerEnv`, `OxcTransformer`,
  `TransformInput`, `Transformer as _`.
- New `lower_module_to_js(code, resolved, env, diagnostics)` helper
  builds a `TransformInput`, runs the transformer, extends the
  bundle's diagnostic vector with anything the compiler emitted.
- New `lang_of(path)` maps `ts` / `tsx` / `jsx` / `mjs` from the
  extension; `compiler_env(env)` maps `BundleEnv` → `CompilerEnv`.
- `bundle_impl`'s chunk-emission loop now calls
  `rewrite_imports` then `lower_module_to_js`, pushing the lowered
  text into `body` instead of the raw source.
- `BUNDLER_BUILD_SECRET` is a fixed placeholder for the compiler's
  server-fn stub pass; threading a per-build secret through
  `BundleOptions` is a follow-up (registered below).

**Tests (`crates/bundler/tests/bundler_jsx_lowering_tdd.rs`)**:

Three tests that shell out to `node --check` on every emitted `.js`
chunk under `dist/`:

| Test | What it covers |
| --- | --- |
| `ssr_chunk_emits_plain_js_for_tsx_page` | TSX page with `interface Props` + JSX return, SSR env |
| `ssr_chunk_emits_plain_js_for_ts_loader` | TS loader with `export interface` + `Promise<LoaderData>` return type, SSR env |
| `client_chunk_emits_plain_js_for_tsx_page` | TSX page with JSX and `className`, client env |

Each walks `dist/` and asserts `node --check` succeeds on every `.js`
file it finds, guarding the exact failure from the handoff.

### Test coverage added

| Suite | Result |
| --- | --- |
| `bundler_jsx_lowering_tdd` | 3 tests, new |
| `bundler_tdd` (existing) | 20 tests, still green after wiring |
| `bundler_rolldown_tdd` (existing) | 8 tests, still green |
| `compiler_tdd` | 39 tests, 2 assertions updated to codegen-stable form |
| `cargo test --workspace` | green |
| `pnpm run test:packages` | 214 / 214 |
| `pnpm test:framework-adapter` | green |
| `pnpm test:smoke` | 5 / 5 |
| `cargo test -p oxide-adapters` | 25 + 5 |

### Verified end-to-end

`cd examples/smoke-app && cargo run -p oxide-cli -- build` emits
server chunks whose first line is:

```js
import { jsx as _jsx } from "react/jsx-runtime";
```

The dynamic-route chunk is equally clean:

```js
import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
export default function Post(props) {
	const title = props.data?.title ?? "Untitled";
	const body = props.data?.body ?? "";
	return /* @__PURE__ */ _jsxs("article", { children: [...] });
}
```

Every emitted `.js` under `dist/server/` and `dist/client/` passes
`node --check`.

### What remains (documented, not blocking)

1. **Vue / Svelte / Solid server compilation in the bundler.** The
   Oxc lowering covers TS + JSX only. Vue SFCs, Svelte components, and
   Solid's `babel-preset-solid` transforms need their respective
   compilers wired through the tooling pool. The React path (which
   covers the smoke-app) is complete.
2. **Per-build secret for server-fn stubs.** `BUNDLER_BUILD_SECRET`
   in `crates/bundler/src/lib.rs` is a fixed placeholder. Add a
   `build_secret` field to `BundleOptions`, generate it once per
   `cmd_build`, and thread it through `lower_module_to_js`.
3. **V8Host migration for `render_pool`** — post-MLP, gated on
   `deno_node` integration for npm resolution.
4. **Playwright E2E for islands hydration.**
5. **Conformance §22: CLI-level E2Es** against the smoke-app.
6. **Deployment adapters** for Deno Deploy, Cloudflare Pages, and
   Vercel's `functions` layout.

### Disk note

`cargo test --workspace` on this machine continues to hold at ~2-4
min with a warm target. The dSYM cleanup scripts in git history remain
the escape hatch if free space gets tight again.

## Session 8 — follow-ups from §7 land; host memory budget documented

**Date:** 2026-10-14
**Scope:** every non-spec-gated follow-up listed in HANDOFF.md §7.

### Landed

| Slice | Commit | Summary |
| --- | --- | --- |
| Bundler TS/JSX lowering | `a3ba56b` | `OxcTransformer` runs `lower_to_js` (Oxc parse → semantic → transformer → codegen) at the tail of `transform`; `bundle_impl` calls it on every non-asset module. §3 closed. |
| Per-build server-fn secret | `a133612` | `BundleOptions::build_secret` threaded through `lower_module_to_js`; CLI generates a fresh secret per build (blake3 over nanoseconds + urandom). |
| Vue / Svelte server compilation | `74cd442`, `d89ab42` | Two paths: (a) `oxide build` runs a CLI precompile pass (`crates/cli/src/precompile.rs`) that dispatches `.vue`/`.svelte` through new worker ops `tooling:vue-compile` / `tooling:svelte-compile`; the emitted JS lands in `BundleOptions::precompiled`. (b) `tsx-loader.mjs` gains `.vue`/`.svelte` support so `dev` / `render` / `serve` compile at module-load time. |
| Deno Deploy config | `1bd8749` | `deno.json` with `deploy.entrypoint` emitted alongside `server.ts`. |
| Cloudflare Pages target | `1bd8749` | New `Target::CloudflarePages`; emits `_worker.js` + `_routes.json` + `wrangler.toml` with `pages_build_output_dir`. |
| CLI-level conformance §22 E2E | `9e7d25b` | `smoke_app_emits_valid_js_in_every_chunk` — asserts every emitted `.js` in `dist/` passes `node --check`, both env dirs have at least one JS chunk, server chunks have no `interface`/JSX, deploy dir has the target's characteristic files. |
| Vercel `functions` layout | `365e12c` | New `Target::VercelFunctions`; `vercel.json` uses the modern `functions` block with explicit runtime/memory/maxDuration. |

### Refused (loudly) rather than silently broken

`bundle_impl` no longer emits raw `.vue`/`.svelte` source when the caller
skipped the precompile pass — the `BLD-FRAMEWORK` diagnostic names the
file and points at the CLI path. This is the fail-fast mode for a caller
that bypasses `cmd_build`.

### Verified end-to-end

- `cargo check --workspace` green.
- `cargo test -p oxide-bundler`, `-p oxide-adapters`, `-p oxide-compiler` green.
- On the session host (4 GB RAM / 4 GB swap) the full workspace test
  binary plus the CLI test binary (each links V8 via `deno_core`) cannot
  co-exist: `cargo test --workspace` SIGKILLs with `signal 9`. Running
  single-crate (`-p <crate> -j 1`) works. This is a host constraint, not
  a code problem.
- Manual `target/debug/oxide-cli build examples/smoke-app`: emitted
  server chunks are `import { jsx as _jsx } from "react/jsx-runtime";`
  and every chunk passes `node --check`.

### Still open (spec-gated, non-blocking)

1. **V8Host migration** for `render_pool` — gated on `deno_node`
   integration; spec calls it post-MLP.
2. **Playwright E2E** for islands hydration — needs a browser install;
   not runnable on this host.
3. **Real sourcemaps** in the bundler (currently empty `{}` — rolldown
   backend will supply them).

### Host memory workaround

Add to `~/.cargo/config.toml` or the repo's `.cargo/config.toml`:

```toml
[build]
jobs = 1

## Session 8 — follow-ups from §7 land; host memory budget documented

**Date:** 2026-10-14
**Scope:** every non-spec-gated follow-up listed in HANDOFF.md §7.

### Landed

| Slice | Commit | Summary |
| --- | --- | --- |
| Bundler TS/JSX lowering | `a3ba56b` | `OxcTransformer` runs `lower_to_js` (Oxc parse → semantic → transformer → codegen) at the tail of `transform`; `bundle_impl` calls it on every non-asset module. §3 closed. |
| Per-build server-fn secret | `a133612` | `BundleOptions::build_secret` threaded through `lower_module_to_js`; CLI generates a fresh secret per build (blake3 over nanoseconds + urandom). |
| Vue / Svelte server compilation | `74cd442`, `d89ab42` | Two paths: (a) `oxide build` runs a CLI precompile pass (`crates/cli/src/precompile.rs`) that dispatches `.vue`/`.svelte` through new worker ops `tooling:vue-compile` / `tooling:svelte-compile`; the emitted JS lands in `BundleOptions::precompiled`. (b) `tsx-loader.mjs` gains `.vue`/`.svelte` support so `dev` / `render` / `serve` compile at module-load time. |
| Deno Deploy config | `1bd8749` | `deno.json` with `deploy.entrypoint` emitted alongside `server.ts`. |
| Cloudflare Pages target | `1bd8749` | New `Target::CloudflarePages`; emits `_worker.js` + `_routes.json` + `wrangler.toml` with `pages_build_output_dir`. |
| CLI-level conformance §22 E2E | `9e7d25b` | `smoke_app_emits_valid_js_in_every_chunk` — asserts every emitted `.js` in `dist/` passes `node --check`, both env dirs have at least one JS chunk, server chunks have no `interface`/JSX, deploy dir has the target's characteristic files. |
| Vercel `functions` layout | `365e12c` | New `Target::VercelFunctions`; `vercel.json` uses the modern `functions` block with explicit runtime/memory/maxDuration. |

### Refused (loudly) rather than silently broken

`bundle_impl` no longer emits raw `.vue`/`.svelte` source when the caller
skipped the precompile pass — the `BLD-FRAMEWORK` diagnostic names the
file and points at the CLI path. This is the fail-fast mode for a caller
that bypasses `cmd_build`.

### Verified end-to-end

- `cargo check --workspace` green.
- `cargo test -p oxide-bundler`, `-p oxide-adapters`, `-p oxide-compiler` green.
- On the session host (4 GB RAM / 4 GB swap) the full workspace test
  binary plus the CLI test binary (each links V8 via `deno_core`) cannot
  co-exist: `cargo test --workspace` SIGKILLs with `signal 9`. Running
  single-crate (`-p <crate> -j 1`) works. This is a host constraint, not
  a code problem.
- Manual `target/debug/oxide-cli build examples/smoke-app`: emitted
  server chunks are `import { jsx as _jsx } from "react/jsx-runtime";`
  and every chunk passes `node --check`.

### Still open (spec-gated, non-blocking)

1. **V8Host migration** for `render_pool` — gated on `deno_node`
   integration; spec calls it post-MLP.
2. **Playwright E2E** for islands hydration — needs a browser install;
   not runnable on this host.
3. **Real sourcemaps** in the bundler (currently empty `{}` — rolldown
   backend will supply them).

### Host memory workaround

Add to `~/.cargo/config.toml` or the repo's `.cargo/config.toml`:

```toml
[build]
jobs = 1
```

and run `cargo test -p <crate>` rather than `--workspace`. Serializes
link steps and cuts peak RSS below the 4 GB ceiling.

## Session 9 — the three remaining §7 follow-ups

**Date:** 2026-10-14
**Scope:** the three items WORKLOG session 8 flagged as still open.

### Landed

| Item | Commit | Summary |
| --- | --- | --- |
| Real sourcemaps | `9746ab5` | Rolldown owns sourcemaps (`SourceMapType::File`). `cmd_build` now picks `RolldownBundler` for TS/JSX projects (real V3 maps, Rolldown's built-in Oxc lowering) and falls back to `ChunkBundler` only when a `.vue`/`.svelte` entry is present, since RolldownBundler doesn't consume `precompiled` yet. `RolldownBundler::convert` writes `<out_dir>/ssr/index.js` (the adapter entry the serverless handlers import) — it was previously only produced by ChunkBundler. |
| V8Host selectable | `6cfa6d7` | `ServerConfig::backend` (`JsBackend::{Node,V8}`). `build_framework_stack` constructs `NodeHost` or `V8Host` accordingly; `cmd_dev` / `cmd_build` / `cmd_serve` pass it through. V8 is spec-honest: the pool builds, `JsRuntime` bootstraps, `render` and `tooling:compile` dispatch through built-in ops; user-module ops return `JSH-NOKIND` until `deno_node` module loading lands. |
| Playwright E2E | `???` | `tests/playwright/` — real Playwright suite booting `oxide start` against the smoke-app, driving Chromium. Not in the default `pnpm test` (host RAM). Asserts HTTP contract, live-DOM equivalence, no console errors. Islands hydration scoped out — no island fixture yet. |

### What the "spec-gated" language actually means

- **V8Host** — the *mechanism* is complete and tested (pool spawns, bootstrap runs, dispatch table exists). The *default flip* is gated on `deno_node` giving us module loading. Users can opt in today via `server.backend = "v8"`.
- **Real sourcemaps** — Rolldown was already capable; the CLIs were just using the in-house bundler. Now fixed.
- **Playwright** — the suite is complete; running it requires a host with enough RAM to launch Chromium.

### Remaining genuinely-open items

1. **Island fixture for Playwright** — the current suite has no `<ox-island>` to hydrate. A follow-up adds one to smoke-app (or a new fixture) and asserts hydration in the browser.
2. **Rolldown plugin hook for `precompiled`** — `ChunkBundler` still serves SFC projects because RolldownBundler doesn't consume the precompiled map. Wiring that would let one bundler serve all projects.
3. **`deno_node` module loading for V8Host** — upstream, not ours.
