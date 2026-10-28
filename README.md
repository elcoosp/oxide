<div align="center">
  <img src="docs/logo.png" alt="Oxide" width="180"/>
  <p>
    <strong>One Rust core. Every framework.</strong><br/>
    A framework-agnostic full-stack meta-framework with a <strong>Rust core</strong>. React, Vue,
    Svelte and Solid are first-class adapters over one shared engine that owns routing, data,
    caching, bundling, serving and deployment — so anything framework-neutral behaves
    identically on all four. Built against the <em>Oxide Technical Specification v0.1</em>.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-stable%20%7C%20edition%202024-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-blue?style=flat-square" alt="License"/>
    <img src="https://img.shields.io/badge/Platforms-Linux%20%7C%20macOS%20%7C%20Windows-9D4EDD?style=flat-square" alt="Platforms"/>
    <img src="https://img.shields.io/badge/Node-%E2%89%A524-339933?style=flat-square&logo=node.js" alt="Node"/>
    <img src="https://img.shields.io/badge/Adapters-React%20%7C%20Vue%20%7C%20Svelte%20%7C%20Solid-007ACC?style=flat-square" alt="Adapters"/>
    <img src="https://img.shields.io/badge/Workspace-17%20crates%20%2B%207%20packages-6F4E37?style=flat-square" alt="Workspace"/>
    <img src="https://img.shields.io/badge/Code-%E2%89%88113k%20lines-9CF?style=flat-square" alt="Lines of code"/>
    <img src="https://img.shields.io/badge/Tests-1239%20Rust%20%7C%20445%2F445%20TS-brightgreen?style=flat-square" alt="Tests"/>
    <img src="https://img.shields.io/badge/ODF-16%20golden%20fixtures-00BFFF?style=flat-square" alt="ODF golden fixtures"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero dependencies"/>
  </p>
  <p>
    <strong><a href="#features">Features</a> · <a href="#quick-start">Quick start</a> · <a href="#the-cli">The CLI</a> · <a href="#architecture">Architecture</a> · <a href="#framework-adapters">Adapters</a> · <a href="#one-wire-format-odf">ODF</a> · <a href="#testing">Testing</a> · <a href="#project-status">Status</a></strong>
  </p>
</div>

## Features

- **One engine, four frameworks.** A versioned adapter contract (`contract: 1`) defines a
  capabilities manifest — suspense mode, islands support, HMR strategy — plus render,
  hydrate, head, loading and plugins hooks. Each adapter ships a zero-dependency neutral IR
  renderer *and* a bridge to the real upstream renderer (`react-dom/server`,
  `@vue/server-renderer`, `svelte/server`, `@solidjs/web`). Framework libraries are optional
  peer dependencies: when they are absent, the adapter degrades gracefully to the IR renderer.
  **Real streaming SSR is the factory default** — `createAdapter()` with no options reaches
  real SSR, and the §14.1 literal config (a routes directory, no config-side imports) is
  pinned by a gate test in every adapter: page, layout and error-boundary components are
  collected at build time into the emitted SSR adapter entry, which both the serve path and
  `oxide render` consume. Island hydration runs the real framework runtime on all four
  adapters, and every adapter emits `<html lang>` from the resolved locale on page and error
  shells alike.
- **Fast paths that skip JavaScript.** The route table is a build-time compiled route tree
  (the `rkyv` archive is built and proven byte-equal; serving uses the in-memory tree).
  Redirects, rewrites, headers, basic auth and rate limits are evaluated natively in Rust.
  Typed config is statically extracted from `oxide.config.ts` — the config path itself never
  evaluates JavaScript.
- **One wire format.** ODF, the Oxide Data Format, is a devalue-compatible superset encoded
  natively in Rust and decoded — and re-encoded — by a single-file, zero-dependency
  TypeScript module. The same bytes go to the cache and to the wire: `'use cache'` values are
  stored ODF-encoded, so `Date`, `Map`, `Set` and `BigInt` survive a cache round trip, on the
  Node and V8 backends alike. Sixteen golden fixtures lock the format byte-exactly on both
  sides, and eight encoder goldens lock the client-side `Date` / `BigInt` / `Map` / `Set` /
  `URL` / `RegExp` / typed-array layout against the Rust encoder byte for byte.
- **Content-addressed to the byte.** A demand-driven incremental graph with early cutoff, a
  `blake3` content-addressed store on disk (`redb` index, `zstd` blobs), byte-stable manifests
  and typegen, and a three-tier cache — L0 in-process response cache, L1 data cache, L2
  pluggable remote handler — with tags, stale-while-revalidate, single-flight, build seeding
  and MAC-sealed remote payloads on all three remote drivers (Redis, S3, HTTP).
- **A single binary.** A `hyper` 1.x + `rustls` server (HTTP/1 + HTTP/2, with HTTP/3 over
  QUIC and automatic `Alt-Svc` advertisement on the TLS listener) with precompressed
  static serving (zstd › brotli › gzip), range requests, streaming responses with dynamic
  compression and `Server-Timing`, zero-downtime reload via `SO_REUSEPORT` (now audit-tested),
  and §5.6 dev cancellation — a mid-flight file change aborts stale dispatches with a
  `x-oxide-stale-generation` 503 instead of serving stale computations. Everything that must
  run JavaScript does so behind one `JsHost` trait: a `deno_core` V8 isolate pool with
  build-time startup snapshots, or a Node child-process pool — both speaking the same op
  protocol, with 8 MiB / 30 s bounded chunk collects (`JSH-OVERFLOW` / `JSH-DEADLINE`) and
  mid-stream worker crashes surfacing as `JSH-CRASH` rather than silent partial documents.
  `--offline-plugins` runs JS plugins and the tooling pool with no network: the flag flows
  from CLI to pool config to every worker child, where the fetch shim refuses egress with
  `JSH-OFFLINE` (spec §17.2, tested in both directions).
- **Deploy anywhere.** Twelve deploy targets — Binary, Node, Static, Docker, Cloudflare,
  Cloudflare Pages, Lambda, Vercel, Vercel Functions, Netlify, Deno, Bun — behind a capability
  matrix that refuses at build time a configuration the target cannot honor. Cloudflare
  targets now bundle a real `dist/deploy/edge_bundle.js` (imported by the shipped
  `edge_handler.js`) and compile `core-wasm` for `wasm32` under a dedicated CI gate.
- **Security and observability built in.** CSP and CSRF modes, SSRF allow-lists, trusted
  proxies, token-bucket rate limits, encrypted session cookies (ChaCha20-Poly1305 AEAD with
  key rotation, decoded into `locals.session`, configurable `security.sessionTtlSecs`), ODF
  decode hardening with prototype-pollution guards. Prometheus-format metrics, W3C
  trace-context propagation into outgoing fetches, actions and endpoints (the `/__data` rows
  carry the request `trace_id`), stage spans and opt-in OTLP export.
- **Platform services included.** Service-worker auto-registration with a build-generated
  manifest-derived precache (including the offline fallback page), stale-while-revalidate and
  build-id-scoped caches, i18n content negotiation with boot-warmed locale catalogs and
  route-subset merging, signed image-transform URLs with a runtime result cache (blake3-keyed
  ETag/immutable replay, concurrency cap with 503 + `Retry-After`, bounded eviction, LQIP
  headers), and an Open Graph image endpoint — `/_oxide/og` renders title/subtitle cards to
  PNG through a swappable backend behind a token-bucket rate limit.
- **Conformance-gated.** One reference app (Atlas), one assertion set, all four adapters.
  A release is blocked the moment any adapter drifts.

## Quick start

> [!NOTE]
> Prerequisites: a **Rust stable** toolchain and **Node ≥ 24**. The TypeScript packages have
> zero runtime npm dependencies and run on Node's native type stripping — no transpile step.

Build the single binary and sanity-check the environment:

```sh
git clone https://github.com/elcoosp/oxide
cd oxide

cargo build --release -p oxide-fw-cli
target/release/oxide doctor
```

Run the smoke app end to end:

```sh
cd examples/smoke-app-react

../../target/release/oxide routes   # scan routes, emit typegen
../../target/release/oxide build    # bundle, prerender, emit deploy artifacts
../../target/release/oxide start    # serve /, /blog/:slug, /_oxide/health
```

Or scaffold a fresh app — 4 frameworks × minimal / blog / dashboard / storefront
(16 templates, each shipping a streaming `defer()` demo and `ssr: 'real'`):

```sh
node packages/create-oxide/src/cli.ts blog svelte my-app
```

Run the test suites:

```sh
pnpm install
cargo test --workspace      # Rust: 17 crates
pnpm test:packages          # TypeScript: 6 packages + conformance
```

> [!TIP]
> `oxide schema` prints a JSON Schema for `oxide.config.ts` — `schemas/oxide-config.schema.json`
> is checked in, regenerated by `oxide-schema`, and byte-for-byte drift-guarded in CI.
> Reference it via `$schema` for editor completions. End-to-end browser tests need a Chromium
> install: `pnpm test:playwright:install && pnpm test:playwright`.

## The CLI

One binary, twenty subcommands:

| | Command | What it does |
| --- | --- | --- |
| **Core loop** | `dev` | Dev server + watcher + HMR hub + inspector; per-file invalidation, idle pre-warm, stale-dispatch cancellation; `--https --qr --open --proxy` |
| | `build` | Client+SSR bundles, prerender, typegen, route manifest, V8 snapshot, L0 cache seed, deploy artifacts (incl. the Cloudflare edge bundle), forbidden-import guard |
| | `start` · `preview` | Production server (pipeline below); health at `/_oxide/health`, readiness at `/_oxide/ready` (503 while the renderer warms) |
| | `render <route>` | Dispatch a page render through the same resolver-supplied component path as the server, HTML to stdout (`--url --param --data --stream`) |
| **Introspection** | `routes` | Scan routes, infer render modes, emit typegen, print the route table |
| | `analyze` | Per-route JS bytes vs `config.budgets` — exits non-zero on breach; top modules by reachable bytes |
| | `check` | Config + route diagnostics, native-plugin gates from `enables()`; typechecks (tsgo) and lints (oxlint) when on PATH — loud `SKIPPED` otherwise |
| | `why <module>` | Which routes import a module, which chunks contain it |
| | `doctor` | 7 environment checks: node, cargo, config, routes dir, cache dir, secrets, cookie keys |
| **Authoring** | `generate` | Scaffold route / layout / action files per framework |
| | `add <module>` | Register a module in `.oxide/modules.json`, reporting its native-plugin gates |
| | `schema` | JSON Schema for `oxide.config.{ts,json}` (checked-in artifact + drift test) |
| | `config` | Load the resolved config, print the framework adapter descriptor |
| | `migrate <source>` | Migration codemods for Next, Nuxt, SvelteKit, Vite (`--write` to apply on disk) |
| **Operations** | `secrets rotate` | Drop the build secret; the next build mints a fresh one |
| | `cache inspect\|clear\|stats` | Inspect, clear or measure `.oxide/cache` |
| | `audit` | Advisory pass over the lockfile via live OSV queries (24h cache); `--json` for a stable JSON envelope |
| | `test` | Build-manifest gate for the app test helpers |
| | `mcp` | stdio MCP server exposing `oxide_routes`, `oxide_cache`, `oxide_graph`, `oxide_errors` |

## Architecture

The request pipeline is ordered so the hot path completes **without entering JavaScript**
whenever the answer is cached, redirected, rewritten or static:

```text
request → hyper 1.x → limits + request id → static assets (precompressed, hashed, range, ETag)
        → native rules (redirects · rewrites · headers · basic auth · rate limits)
        → route match (compiled tree)
        → L0 response cache (weak-ETag 304s · stale-while-revalidate · single-flight · build-seeded)
        → JS middleware (runs before loaders/actions, injects event.locals, may short-circuit
          or rewrite; short-circuits persist session mutations too)
        → loaders: layout chain outer→inner (await parent()), then the page load — the last
          layout and the page load run in parallel when the page never calls parent();
          deferred values resolve and reach the client as a nonce'd script
        → dispatch: render | action | endpoint (route.ts) | server fn
        → streaming response (real SSR chunks) + Server-Timing + dynamic compression
        → cache fill + tag invalidation (refuses cookie-bearing responses on both the PPR
          shell and L0 paths; shells are build-id-scoped with one-at-a-time eviction)
```

```text
crates/         cli · config · graph · compiler · bundler · jshost · router · server ·
                render · cache · odf · assets · dev · plugin · adapters · core-wasm ·
                diagnostics                                   (17 crates, ~88k lines)
packages/       @oxide-fw/core (runtime) · @oxide-fw/react · @oxide-fw/vue
                · @oxide-fw/svelte · @oxide-fw/solid · create-oxide ·
                oxide-website   (runtime pkgs: zero deps)
conformance/    Atlas reference app — one assertion set × four adapters
examples/       smoke-app-{react,vue,svelte,solid} — end-to-end fixtures
benchmarks/     method, harness and the >5% CI regression gate
tests/          Playwright specs + fixture check scripts
schemas/        oxide-config.schema.json (checked in, CI drift-guarded)
docs/           architecture.md · spec-v0.1.md (frozen) · spec-v0.2-draft.md · deviations.md
```

Every external engine sits behind an isolation trait — `JsHost`, `Bundler`, `Transformer`,
`CssBackend`, `ImageBackend`, `FontBackend` — so call sites never change when an engine does.
CSS runs on Lightning CSS or an in-house backend, images on a pure-Rust AVIF/WebP/JPEG/PNG
pipeline with signed URLs, WASM plugins on a sandboxed `wasmtime` component host with no
implicit WASI preopens. Framework tooling (Vue SFC, Svelte, Solid JSX, Tailwind candidates)
routes through a tooling pool driving the real upstream compilers.

## Framework adapters

| Adapter | Package | Extensions | Suspense | HMR strategy |
| --- | --- | --- | --- | --- |
| React | `@oxide-fw/react` | `.tsx` | native | refresh |
| Vue | `@oxide-fw/vue` | `.vue` | core | component |
| Svelte | `@oxide-fw/svelte` | `.svelte` | core | component |
| Solid | `@oxide-fw/solid` | `.tsx` | native | component |

Rendering, hydration and HMR strategy are the only things an adapter owns. The rest of the
client runtime is core-owned (`packages/oxide`) and shipped identically to every framework:
the ODF decoder, the island loader (strategies `load · idle · visible · media(q)` on
`<ox-island>` hosts), progressive form `enhance`, script strategies, deferred-boundary glue,
navigation loading boundaries (`onNavigateStart/End` + per-adapter `renderLoading`),
manifest-driven chunk preloading on prefetch/goto that now walks the **layout chain**
(`layoutChunksFor`), and a Core Web Vitals reporter that beacons to `/_oxide/vitals` with
zero third-party script. Island hydration calls the real framework runtime on all four
adapters (`createSSRApp`/`mount(hydrate)`/`hydrate`/`createRoot`).

## One wire format: ODF

ODF is a version-prefixed, type-tagged, little-endian binary format — a devalue-compatible
superset covering JSON primitives plus `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`,
typed arrays, `undefined`, cyclic and shared references, `Error` and deferred promise slots.

- Encoded natively in Rust (`oxide-odf`), decoded by a single-file zero-dependency TypeScript
  module with hardening parity: 256 MiB input cap, depth 512, 1M refs, `__proto__` /
  `constructor` rejection, stable `ODF-*` error codes.
- Sixteen golden fixtures are committed as bytes and asserted **byte-exact** by both the Rust
  encoder and the TypeScript decoder — the format cannot drift between sides.
- JSON-LD entries render as their own `<script type="application/ld+json">` through the
  shared head-merge pipeline on every adapter.

## Testing

The repository is built test-first: each crate's `*_tdd.rs` suites were written against its
contract before the implementation.

- **1,215 Rust test functions** across 17 crates (27 gated behind bare `#[ignore]`, 29
  more annotated with reasons, plus env-gated suites with stated reasons — the Node-loader e2e suite runs in CI's `jshost-e2e`
  job), including `proptest` property suites on the router and graph engines,
  `cargo-fuzz` harnesses for the ODF decoder, router matcher, config extractor and path
  splitter, acceptance tests for the cache-security closures (single-flight owner-drop
  regression, tampered-entry rejection, per-env bundle-skip flips driven through real
  rebuilds) and the defer-streaming wave (per-settle flush with the close tag withheld and
  re-emitted last, close tags split across chunks, no-settle byte-identity, and client-side
  late-settle / rejection / dispose slot resolution), plus the PPR/session hardening suite
  (cookie-refusal for both loader and middleware cookies on the shell **and** L0 paths,
  build-scoped shell keys, oldest-entry eviction, `/__data` no-reissue for untouched
  sessions, worker session-on-throw) and the §5.6 stale-dispatch cancellation tests.
- **445 TypeScript test cases** across 74 files in the publishable packages — 475 total
  with the 32-assertion conformance suite, all green at this commit — on Node's built-in
  test runner, including
  the ODF decoder against the same golden fixtures as the Rust encoder (typed values:
  `BigInt`, `Date`, `Map`, `Set`, `URL`, `RegExp`, typed arrays), router / island
  tests against a hand-rolled fake DOM (no jsdom, no deps), suites for the defer script
  protocol and client-side loading boundaries, the per-adapter `<Image>` components,
  the client message-catalog loader with route-subset merging, the content-collections
  runtime (entry shape, content-hash caching, root resolution), the route-search client
  scorer (tokenize → prefix match → field-weighted, byte-stable ties), and the §14.1
  factory-default gate (`createAdapter()` with no options reaches real SSR) run in all four
  adapter packages.
- **Conformance gate** — the Atlas reference app runs one 24-assertion set (contract shape,
  SSR content + framework markers, merged head, deterministic markup, streaming boundaries,
  deferred boundaries in settle order, error / not-found, islands, actions JS-on/off, server
  functions, cache shapes, client router, prefetch) against all four adapters: 96 checks per
  release, and 21 Rust snapshot tests lock the emitted HTML and deploy artifacts byte-for-byte.
- **Playwright** — 26 specs across 11 files, run per browser (Chromium, Firefox, WebKit) that boot the real binary and drive a
  real browser: SSR identity with zero console errors, island hydration (click 3 → 4 → 5),
  navigation loading boundaries, middleware locals + redirect, socket round-trip and
  two-client isolation, ordered SSE streaming, and the config-gated `/_oxide/og` endpoint
  (PNG magic bytes, 400 on a missing title, byte-determinism).

CI runs **sixteen gates**: Rust fmt + tests on a Linux / macOS / Windows matrix, the
`wasm32` edge-bundle compile gate, Node 24 package tests, the conformance suite, the V8
fixture check, a plugin-fixture job gating the build-time plugin hooks, a benchmark gate
that **fails the build on >5% regression** against `benchmarks/report.md`, a Redis
service-container job for the L2 integration suite, an ODF fuzz-property job, the
stale-string gate, a **launch-content gate** that greps the website and marketing content
for stale claims and compares this README's counts against `scripts/count-tests.sh`, a
spec-anchor gate, an HMR latency harness that fails on zero successful iterations, the
Node-loader jshost e2e job, a Miri job on the pure-Rust crates, and a Playwright job
running every spec on all three engines, one matrix cell per browser.

## Project status

> [!WARNING]
> Oxide is pre-alpha software under active development toward
> [spec v0.1](docs/spec-v0.1.md). The adapter contract is versioned and treated as frozen;
> everything else may churn.

**Snapshot 2026-10-24, `15f7e1f`.** Counts in this section come from
[`scripts/count-tests.sh`](scripts/count-tests.sh) and the `launch-content` gate —
hand-maintained numbers rotted four rounds running, so the prose now carries no
count the gate cannot reproduce.

### Shipped and CI-gated

- **Rendering** — streaming SSR is the factory default on both worker backends: real
  upstream renderers (`react-dom/server`, `@vue/server-renderer`, `svelte/server`,
  `@solidjs/web`), per-settle `defer()` delivery (one nonce'd script per settle, close tag
  withheld and appended last, 30 s settle bound, `OXIDE_DEFER_BUFFERED=1` kill switch,
  browser-side slot resolution riding a `MutationObserver`), real island hydration on all
  four adapters, `<html lang>` on page and error shells, and the PPR hardening suite
  (cookie-refusing shells, build-id-scoped keys, one-entry LRU eviction, route/path tag
  invalidation) behind an experimental runtime shell cache.
- **Server** — hyper 1.x + rustls (HTTP/1+2, HTTP/3 with `Alt-Svc`), native
  redirects/rewrites/headers/basic-auth/rate-limits, precompressed static with range
  requests, §5.6 stale-dispatch cancellation (503 `x-oxide-stale-generation`), session
  cookies (ChaCha20-Poly1305 AEAD, rotation) with write-back parity on every response path
  and `security.sessionTtlSecs` honored in dev and prod, CSP/CSRF/SSRF guards, Prometheus
  metrics, W3C trace-context threaded into loaders/actions/endpoints, and RFC6455 socket
  routes with `ctx.broadcast` / `ctx.sendTo` / `ctx.clients` fan-out and presence —
  Playwright-tested end to end.
- **Data & cache** — ODF byte-locked by golden fixtures on both sides and ODF-encoded
  `'use cache'` on Node and V8, three-tier cache with tags / single-flight / SWR and
  MAC-sealed Redis/S3/HTTP payloads, DemandGraph-backed per-route prerender reuse recorded
  in `.oxide/graph.json`.
- **Build & deploy** — full-layout-chain prerender, six-layer config extraction over a
  CI-drift-guarded JSON Schema, the rkyv route archive proven byte-equal, 12 deploy targets
  behind a build-time-refusing capability matrix, and the Cloudflare edge chain:
  embedded `edge.wasm`, emitted `edge_bundle.js`, consumed by the shipped `edge_handler.js`,
  `wasm32` CI-gated.
- **Content & platform** — content-collections runtime with the shipped website's
  blog/docs loaders as its first real consumers, a build-generated service worker with an
  offline fallback, the route-search index, the signed image pipeline (`<Image>` with
  srcset-derived `sizes`), locale catalogs with client-side ICU evaluation, font
  subsetting, MDX, and the config-gated `/_oxide/og` endpoint behind a token bucket.
- **Tooling** — `dev` with HMR (loader edits and route rescan included), `check`
  (tsgo + oxlint), `audit` (live OSV), `add`, `migrate` (working codemods), `doctor` /
  `why` / `analyze` / `schema` / `mcp`, typed routes with locale-aware `href()`, all 19
  plugin hooks through the real bundler, and `--offline-plugins` proven in both
  directions.

### Measured today

| What | Value | Source of truth |
|---|---|---|
| Rust tests | 1239 | `scripts/count-tests.sh` (static attr count) |
| TypeScript tests | 445 packages + 32 conformance = 477, green | suites run + conformance run |
| CI gates | 16 jobs | `.github/workflows/ci.yml` |
| Browser e2e | 26 specs / 11 files × Chromium, Firefox, WebKit | `tests/playwright` |
| Spec deviations | 2 open, 4 closed | [`docs/deviations.md`](docs/deviations.md) |

### Known broken at this commit

- **The V8 backend is down here** — an authored JS syntax error in the plugin bootstrap
  (`crates/jshost/src/lib.rs:1269`, from `0574ab8`) parse-fails every snapshot-less
  isolate; upstream CI is red because of it. The Node backend (the default) is unaffected.
  This bullet is deleted by the fix commit — the v0.1 release plan gates the release on
  green CI, and nothing else.

### Honest gaps

Two open spec deviations, both documented and TDD-locked in
[`docs/deviations.md`](docs/deviations.md): per-chunk incremental bundling ships as
env-level reuse (5-A), and font output is subsetted TrueType rather than WOFF2 (16-B).
Four earlier deviations closed with tests: dev route rescan, edge `mergeHead` in Rust,
embedded edge wasm, and `<Defer>` for `suspense: 'core'` adapters. Partial prerendering
is a runtime shell cache for `defer()` boundaries — deliberately not the post-MLP §10.4
design, and described nowhere as otherwise. The edge wasm payload is embedded and
preferred-when-loaded; an execution-level proof of that path is pending in the v0.1
release plan. [`WORKLOG.md`](WORKLOG.md) carries the session-by-session history (the
2026-10-19 `HANDOFF.md` is a superseded snapshot).

### What remains for v0.1

A short, bounded list, tracked in [`docs/release-plan-v0.1.md`](docs/release-plan-v0.1.md):
un-red CI (the V8 one-liner above plus its triage day), the swapper/ICU/action correctness
trio, this document's numbers becoming fully gate-generated, and the tag/publish mechanics
that have never been dry-run. Five waves, each ending green, ~1 working week. No
structural work remains.

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — spec § → crate map, pipeline and
  cross-cutting invariants
- [`docs/spec-v0.1.md`](docs/spec-v0.1.md) — the frozen v0.1 spec (audit target);
  [`docs/spec-v0.2-draft.md`](docs/spec-v0.2-draft.md) — the v0.2 implementation edition
  (work packages, waves, appendices); the combined original,
  [`docs/Oxide Technical Specification.md`](docs/Oxide%20Technical%20Specification.md), is
  content-frozen with a version-note banner so existing line anchors resolve
- [`docs/deviations.md`](docs/deviations.md) — the numbered, TDD-locked spec deviations
  (six entries; four closed, two open — the current-state doc of record)
- [`docs/release-plan-v0.1.md`](docs/release-plan-v0.1.md) — the v0.1 release plan:
  five criteria, eight blockers with acceptance checks, five waves
- [`schemas/oxide-config.schema.json`](schemas/oxide-config.schema.json) — the checked-in
  config JSON Schema (regenerate via `oxide-schema`; CI fails on drift)
- [`docs/plugin-compat.md`](docs/plugin-compat.md) — plugin kinds × hooks compatibility
  matrix (what is wired, partial, or unsupported per plugin kind)
- [`benchmarks/README.md`](benchmarks/README.md) — benchmark method and per-route JS budgets
- [`conformance/`](conformance/) — the Atlas reference app and the cross-adapter suite
- [`examples/smoke-app-react`](examples/smoke-app-react) — the minimal app used in the
  end-to-end smoke run
