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

## Session 10 — islands in a browser, four fixtures, native image endpoint, server functions, 'use cache'

**Date:** 2026-10-14
**Scope:** everything since session 9. Seventeen commits; every slice
verified by running it, not by inspection.

This entry is long because the session was: it starts at the island
feature and ends at "server functions work end-to-end against a real
`oxide start`". It is grouped by feature, each with its proof.

### 1. Islands — from markup to a real browser click

Commits `756a9d2`, `cae43af`, `f1f1f8c`, `b59f733`, `59dc9e8`,
`27ab15e`.

The pieces existed in isolation. Making them work surfaced six distinct
bugs, in the order a user would have hit them:

1. **The boot script tag was injected but `/client/*` was not served.**
   `oxide start` builds a pipeline with `assets: None`; the emitted
   `dist/client/*.js` had no route. Fixed by loading `dist/` into the
   static-asset store (`/client/x.js` → `<dist>/client/x.js`).

2. **Live responses did not carry the boot script.** `oxide build`
   injects it into prerendered files; `oxide start` renders through
   `build_dispatch`, which had no knowledge of islands. Fixed by
   threading the boot URL through and splicing the tag into the
   streaming response.

3. **The streaming splice put the tag after `</body>`.** The position
   was `i + CLOSE.len()` (end of marker) not `i` (start). Browsers
   reflow stray elements so it worked, but the code and the comment
   disagreed. Fixed and moved to `oxide-server::inject_before_close`
   — a generic body-stream transform, tested in a crate that does not
   link V8 (see §"Test location" below).

4. **React's IR `hydrateIsland` cannot hydrate hooks.** It calls the
   component directly with a synthetic `client` prop; a `useState`
   component throws `Cannot read properties of null`. Fixed with
   `react-dom/client`'s `hydrateRoot`. The named `adapter` export stays
   the IR path (the conformance suite drives it against a fake DOM);
   the browser hydration lives on a separate `clientAdapter` export the
   boot entry imports.

5. **`boot.ts` touched `window` at module top level.** `@oxide/react`
   re-exports it, and `oxide.config.ts` imports `@oxide/react` — so it
   was evaluated in the Node worker (`window is not defined`), aborting
   the framework stack before the build ran. Fixed: every DOM access is
   behind a guard, and the module exports `boot()` / `installBootRuntime()`
   instead of auto-running.

6. **The CSP blocked the boot script.** `strict-dynamic` ignores
   `'self'`; the tag had no nonce. Worse, `finish_response` derived the
   nonce from a response header that never carried the request id, so
   *every* response got the same nonce (`blake3("")`) — a security hole
   on its own. Fixed: `finish_response` takes the request id; the CLI's
   streaming injector derives the same value. The two now match.

`b59f733` added `NODE_ENV=production` for the client bundle (React's dev
build is ~1 MB) and `sideEffects: false` to the framework packages. The
counter chunk went **960 KB → 6.8 KB**, total client **~1.01 MB → ~38 KB**.

`59dc9e8` runs the whole thing in Chromium: Playwright 7/7, including a
click that advances the counter 3 → 4 → 5.

### 2. Four framework fixtures + Rust-native Solid compilation

Commits `b92ac0d`, `a36d22f`.

`examples/smoke-app` → `examples/smoke-app-react`, joined by
`smoke-app-vue`, `smoke-app-svelte`, `smoke-app-solid`. Each is a real
project — a `.vue`/`.svelte`/`.tsx` page, a neutral `page.server.ts`
loader, an `oxide.config.ts` that imports the page.

Three gaps this exposed:

- **§10.1 mode inference was missing.** Any route with a `page.server.ts`
  was treated as dynamic, so nothing prerendered. `read_page_config` in
  `crates/router` now infers `static` vs `dynamic` from a marker scan
  (cookies, headers, locals, searchParams), and `cmd_build` honors it.
  The scan is a regex, not the source-span compiler pass the spec
  describes — documented as the follow-up.

- **Vue's compiled output kept TypeScript.** `@vue/compiler-sfc` with
  `lang="ts"` emits TS annotations; Node only strips types from files
  with a `.ts` extension, and the loader hands back a `.vue`. Fixed in
  `tsx-loader.mjs` (dev) and the `tooling:vue-compile` worker op
  (build): sucrase `typescript` on the compiler output.

- **Solid's JSX does not compile to React's factory calls.** Solid 2.0
  emits `ssr`-template literals importing from `@solidjs/web`; Oxc's
  automatic runtime produces `jsx()`/`jsxs()` from `react/jsx-runtime`.
  The spec's answer is a framework compiler in the tooling pool. The
  right dependency is **`solid-jsx-oxc`** — a Rust/OXC-native Solid JSX
  compiler with NAPI bindings, from the SolidJS maintainers. No babel.
  The project chose Rolldown + Oxc precisely to avoid a JS compile path,
  and this keeps it that way. `tooling:solid-compile` calls it; the
  `0.1.0-alpha` ignores `moduleName`, so the emitted `solid-js/web`
  specifier is rewritten to `@solidjs/web`.

All four fixtures build and render their own page through their
framework's real SSR renderer:

| fixture | body |
| --- | --- |
| `smoke-app-react` | `<h1>Home</h1>` + `<ox-island data-ox-island="Counter" …>` |
| `smoke-app-vue` | `<h1>Home</h1><p>Hello from Oxide (vue)</p>` |
| `smoke-app-svelte` | `<h1>Home</h1> <p>Hello from Oxide (svelte)</p>` |
| `smoke-app-solid` | `<h1>Home</h1><p>Hello from Oxide (solid)</p>` |

### 3. Test location — a lesson worth writing down

Two slices were rewritten because of *where* the test lived, not what it
asserted.

- The four-fixture conformance check started as a `#[test]` in
  `oxide-cli`'s lib test target. That target statically links V8
  (~140 MB) and relinks on every source change: ~5 minutes per
  iteration, then four Node workers spawning in-process. Moving it to
  `tests/fixtures/check.sh` — a shell script driving the already-built
  `target/debug/oxide-cli` — runs the same four builds in **13 seconds**.

- The streaming injector moved from `oxide-cli` to `oxide-server` for
  the same reason: `oxide-server` does not depend on `oxide-jshost`, so
  its tests compile in seconds instead of minutes.

Rule: **if a test only needs the built binary, do not put it inside a
target that links V8.**

Also fixed: `[profile.test] opt-level` was `1`, `[profile.dev]` was `0`.
Different opt-levels mean different rlibs, so the first `cargo test`
after a `cargo check` recompiled the whole dependency tree (ravif,
resvg, tiny-skia, skrifa, lightningcss, comrak, syntect) — minutes of
silent `rustc`. Both are `0` now.

### 4. `/sitemap.xml` and `/robots.txt` (§16.3)

Commit `8f3b504`.

`oxide-assets` already had `sitemap_xml` / `robots_txt`; nothing called
them. Added `OxideConfig.siteUrl` (and `KNOWN_KEYS`), then
`emit_sitemap_and_robots` in `cmd_build`. The sitemap walks the route
table for URL-addressable pages plus `dist/prerendered/**.html` as the
ground truth for "which URL exists" — which is what includes the
runtime-`generateParams` enumerations. React's fixture emits three
absolute URLs (`/`, `/blog/hello-prod`, `/blog/second-post`).

`robots_txt` with no rules now emits an explicit
`User-agent: *\nDisallow:` — the previous empty output was valid but
ambiguous.

### 5. Native `/_oxide/image` endpoint (§16.1)

Commits `fdff7b3`, `b9ff957`.

`oxide-assets` had the transform (`RustImageBackend`) and the signer
(`sign_image_url`); nothing served the route. Added:

- **`oxide-server`** — `ImageEndpoint` + a `/_oxide/image` branch in
  `native_prefix`. Source resolution (a `/`-prefixed path under an asset
  root; canonicalized containment rejects traversal), the optional
  signature check, and the response headers (`Cache-Control: immutable`,
  `ETag` from the backend's cache key, `Vary: Accept`).

- **`oxide-cli`** — `AssetsImageBackend` adapts
  `oxide_assets::ImageBackend` to the server's trait seam, so
  `oxide-server` never depends on `oxide-assets` (which pulls
  ravif/resvg/skrifa/lightningcss). `build_image_endpoint` reads
  `OXIDE_IMAGE_SECRET` for the signing check.

- **Remote source gating** (`b9ff957`) — `remote_pattern_allows` (exact
  host or `*.suffix`; optional protocol), plus the existing §17.1
  `ssrf_allowed`. The fetch itself returns `501` — `serve_request`'s
  native prefix is synchronous and the fetch needs `.await`.

Verified live against `oxide start`:

| request | result |
| --- | --- |
| `?url=/test-red.png` | 200, `image/png`, immutable, ETag |
| `?url=…&w=64` | 200, resized, distinct ETag |
| `?url=…&w=64&format=webp` | 200, WebP (RIFF verified) |
| `?url=/../../etc/passwd` | 404 |
| `?format=jxl` | 400 |
| (no `url`) | 400 |

### 6. Server functions (§9.4) — end-to-end

Commits `65acb8f`, `aa8efad`, `43e28fa`.

Four pieces, three of them new:

- **Client caller** — `packages/oxide/src/fn.ts`, `__oxide_call_fn(id,
  args)`, POSTs to `/_oxide/fn/<id>`. The compiler's stub (both the
  scanner and Oxc paths) emits an import of it plus
  `export const NAME = (async (...args) => __oxide_call_fn("<id>", args));`.
  Previously the stub referenced bare `__oxide_encode`/`__oxide_decode`
  globals nothing defined.

- **HTTP route** — `oxide-server` gains `ServerFnDispatcher` (a trait
  seam) and `NativePrefix::ServerFn`. The pipeline enforces POST-only
  (405 + `Allow`), `application/oxide-fn` content type (415), and CSRF,
  then dispatches on an async arm. The envelope is
  `{"ok":true,"data":…}` / `{"ok":false,"code":…,"message":…}`.

- **Registration** — the compiler appends
  `__oxide_register_fn("<id>", <export>)` to a `.server.ts` on the SSR
  env, but nothing defined that global. `oxide/server` installs it.

- **Worker op + host** — `server-fn:run` imports the SSR adapter entry
  once (its import graph registers every `serverFn`), looks the id up in
  `__oxide_fn_registry`, runs it, returns the value. `ServerFnHost` wraps
  it; the CLI's `ServerFnDispatcherImpl` maps `JSH-FN-NOT-FOUND` → 404,
  `JSH-FN-THREW` → 400.

**The bundler gap this exposed.** `RolldownBundler` deferred to
Rolldown's own Oxc lowering, which strips TS but knows nothing about
`'use cache'`, `serverFn`, or the server-only guard. So an SSR chunk
carried raw `serverFn(async …)` and no `__oxide_register_fn`. Fixed with
`OxidePassesPlugin` (`crates/bundler`) calling a new
`OxcTransformer::transform_passes_only` — the AST passes without the
TS/JSX lower, so Rolldown lowers once, downstream. It skips anything
outside the project root, `node_modules`, and precompiled SFCs.

**And a subtle one.** `bundle_env_options` generated its own per-build
secret on each call, so the client and server bundles hashed *different*
secrets into the same function name — the stub POSTed an id the registry
never held. `cmd_build` now generates one secret for both.

Verified live: `POST /_oxide/fn/<id>` with `["Grace"]` →
`{"ok":true,"data":{"message":"Hello Grace from a server fn"}}`. Unknown
id → 404; `GET` → 405; wrong content type → 415.

### 7. `'use cache'` (§9.3) — directive installed, two limitations

Commit `75d16f1`.

The compiler's `'use cache'` pass emits
`__oxide_cache(fn, module, export, {tags, life})`; nothing defined it.
`oxide/server` installs the global (mirroring `__oxide_register_fn`),
adapting `{tags, life}` to the existing runtime `cache()`.

Verified the emitted shape in a real build:
`__oxide_cache(async e => …, "<abs path>", "getPost", {life:{ttl:60,swr:300}})`.

**Two limitations, both documented in the code:**

1. **Non-literal tags are dropped.** `cacheTag('posts:' + slug)` depends
   on an argument the static extractor cannot evaluate, so `tags` is
   absent from the emitted options. A literal (`cacheTag('posts')`)
   survives. The fix is a thunk: `tags: (...args) => [...]`.
2. **The Rust L1 tier is not on the request path.** The runtime wrapper
   is an in-process `Map`. Cross-request / cross-node caching needs the
   worker `cache_get`/`cache_set` ops (declared in the `Op` table, not
   implemented) bridging to `oxide_cache::CacheHandler`.

### Test state at the end of this session

| suite | result |
| --- | --- |
| `packages/oxide` | 69 |
| `packages/react` | 36 |
| `packages/vue` | 30 |
| `packages/svelte` | 29 |
| `packages/solid` | 29 |
| conformance (`conformance/test`) | 4/4 adapters |
| Playwright | 7/7 (real Chromium) |
| `oxide-compiler` `compiler_tdd` | 39 |
| `oxide-server` `inject_before_close_tdd` | 16 |
| four-fixture CLI gate (`tests/fixtures/check.sh`) | PASS, ~13 s |

### What remains

Ordered by leverage:

1. **§9.3 tags + L1 cache on the request path** — the compiler emits a
   tag thunk, the runtime registers resolved tags, the worker
   `cache_get`/`cache_set` ops bridge to `CacheHandler`. Closes both
   documented limitations in one slice.
2. **§16.1 remote image fetch** — validation done; the fetch needs
   `serve_request`'s native prefix restructured for `.await`.
3. **§15 plugin host wiring** — `crates/plugin/src/wasm.rs` has a working
   `WasmPlugin`; connect it to the bundler's transform chain.
4. **§10.1 mode inference as a real compiler pass** — the current scan is
   a regex; the spec wants source spans and a diagnostic naming the read
   that made a route dynamic.
5. **§21 benchmark harness** — `benchmarks/` is a README; the reference
   apps exist (the four fixtures), the harness does not.
6. **§4.4 app snapshots**, **§12 HTTP/3**, **§19 editor tooling**,
   **§22 full assertion set** — larger, spec-named, not started.

## Session 11 — §9.3 tags, §15 WASM plugins, §16.1 remote images, §10.1 AST inference

**Date:** 2026-10-14
**Scope:** the three features the session-10 entry named as "the natural next
slices" (tags + L1 cache, WASM plugin host, remote image fetch), plus §10.1
inference as a real compiler pass.

### 1. §9.3 `'use cache'` — the tag thunk

Commits `b2ad76e`, `686aa83`, `fb3e736`, `75d16f1`.

The directive was installed but dropped any `cacheTag(…)` argument the static
extractor could not evaluate. `cacheTag('posts:' + slug)` was omitted; the
entry carried no tag.

- **Compiler** — `collect_cache_options` now records each `cacheTag`
  argument as raw source and, when any argument is non-literal, emits a
  thunk over the wrapped function's own parameter list:
  `tags: (slug) => ["posts:" + slug]`. A literal tag stays a plain array,
  so the common case carries no per-call closure.
- **Runtime** — `CacheOptions.tags` accepts `string[] | ((...args) =>
  string[])`; `cache()` resolves the thunk per call, includes the resolved
  tags in the cache key, and stores them on the entry.

Verified in a real build: the emitted wrapper is
`__oxide_cache(async e => …, "<path>", "getPost", { tags: e => [`posts:` + e], life: { ttl: 60, swr: 300 } })`.

### 2. §9.3 — the L1 cache on the request path: **not done**

The runtime `cache()` is an in-process `Map`. Cross-request caching needs the
worker to read the Rust L1 tier while executing user code, and the worker
protocol has no path for that:

- it is one-directional (Rust sends `call`, worker replies
  `call_ok`/`call_err`/`call_chunk`/`call_end`),
- the pool reads frames only when awaiting a reply, so a frame the worker
  sends mid-call has no dispatcher,
- `cache()` would need an async op that sends a `cache_request` frame and
  awaits the reply.

The change is: a new frame type, the pool's read loop becomes a dispatcher
backed by a `CacheHandler`, and the runtime op becomes async. Real work, and
the right next slice. Recorded here so it is not mistaken for a wiring fix.

### 3. §15 WASM plugin host — wired

Commits `b253039`, `f318aa9`, `827e301`, `ecea7cc`, `550d6c0`, `9b856c2`,
`508abb6`, `3a826a5`.

The `oxide-plugin` WASM host worked (real component-model components execute
their `transform` export), but nothing called it, and adding `oxide-plugin`
to the bundler would pull `wasmtime` into every build for a feature most
builds do not enable.

- **`oxide-bundler`** — `BundleOptions.transform_hook: Option<TransformHook>`.
  `TransformHook` wraps `Arc<dyn Fn(&str, &str) -> Result<String>>`, so the
  bundler's only knowledge of a plugin is "call this function". The newtype
  exists because `BundleOptions` derives `Debug` and an `Arc<dyn Fn…>` does
  not.
- **`OxidePassesPlugin`** runs the hook *before* the compiler passes — a
  plugin sees the source as its author wrote it. A hook error is reported
  and skipped.
- **`oxide-cli`** — `build_transform_hook` reads `config.plugins` for
  `{ kind: "wasm", path, … }`, loads the component, wraps its `transform`.
  A plugin that fails to load or exports no `transform` is skipped with a
  warning.

Two bugs in `should_transform` surfaced while wiring: Rolldown's virtual
modules carry a leading NUL (`\0rolldown/runtime.js`), and a resolved module
can be reported under `/private/var/…` while the root was built with
`/var/…` — macOS symlinks one to the other, so `starts_with(root)` fails on
a valid module. Both sides are canonicalized now.

**A deadlock, diagnosed by sampling.** The first transform-hook test used
`assert!` inside the closure. A panic in a plugin hook on Rolldown's
current-thread runtime has no other thread to unwind to, so the executor
parked — the test ran for 28 minutes before `sample` showed every thread in
`__workq_kernreturn`. The hook now records ids and rewrites only when its
marker is present: the shape a real plugin follows, and one that cannot
deadlock the pipeline.

### 4. §16.1 remote image fetch — the other half

Commits `35ecb3c`, `f3dd85e`, `d8a33a6`.

`b9ff957` landed `remote_pattern_allows` and the SSRF check; the endpoint
answered every remote source with 501 because `serve_request`'s native
prefix was synchronous.

- `NativePrefix::RemoteImage { endpoint, request, format, rule_headers }`.
  The sync prefix still does *all* validation; the async arm fetches.
- `fetch_remote_image` uses the same pooled `hyper-util` client
  `forward_proxy` uses, streams the body, and stops at `remote_max_bytes`
  — a source that streams unbounded is refused, not buffered.
- `plan_remote_image` returns the validated request or the rejection;
  `serve_image_endpoint` returns `NativePrefix` now (a remote source needs
  the async variant, a local one `Raw`).

Five tests cover the validation: unlisted host → 403, empty pattern list →
403, loopback / `localhost` → 403 even when matched, non-http scheme → 400,
allowed `https://` accepted.

### 5. §10.1 inference as a compiler pass

Commits `9b540ac`, `0af5c2f`, `7172b09`, `028f6b6`.

The scanner's inference was a line scan: a loader whose file contained the
substring `cookies` read as dynamic, even in a comment or a string literal.

- `oxide_compiler::infer_render_mode(source, path) -> InferredMode` parses
  with Oxc and walks the AST. A read counts only as a member access on
  `cookies`, `locals`, `request.headers`, or `searchParams`. The result
  carries the trigger's span and name.
- `cmd_build` and `cmd_routes` both call it, so the build and the table
  agree by construction.
- `oxide-router` does not grow an `oxide-compiler` dependency for this —
  the CLI already depends on both.

Verified: a loader whose string literal is `'We use cookies to improve your
experience'` read **dynamic** before and **static** after.

### Test state

| suite | result |
| --- | --- |
| `oxide-compiler` | 49 |
| `oxide-server` | 106 across four binaries |
| `oxide-bundler` | 44 across four binaries |
| `packages/oxide` | 69 |
| conformance | 4/4 |
| Playwright | 7/7 |
| four-fixture CLI gate | PASS |

### Still open

1. **§9.3 L1 cache** — the protocol change above.
2. **§21 benchmark harness** — the four fixtures are the reference apps;
   `benchmarks/` is a README.
3. **§4.4 app snapshots, §12 HTTP/3, §19 editor tooling, §22 full assertion
   set** — larger, spec-named, not started.
