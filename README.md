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
    <img src="https://img.shields.io/badge/Workspace-17%20crates%20%2B%206%20packages-6F4E37?style=flat-square" alt="Workspace"/>
    <img src="https://img.shields.io/badge/Code-%E2%89%8899k%20lines-9CF?style=flat-square" alt="Lines of code"/>
    <img src="https://img.shields.io/badge/Tests-1087%20Rust%20%7C%20342%20TS-brightgreen?style=flat-square" alt="Tests"/>
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
  Real streaming SSR is the primary live render path, and island hydration runs the real
  framework runtime on all four adapters.
- **Fast paths that skip JavaScript.** The route table is a build-time radix tree serialized
  to an `rkyv` archive and memory-mapped at boot — matches read the mapped bytes in place.
  Redirects, rewrites, headers, basic auth and rate limits are evaluated natively in Rust.
  Typed config is statically extracted from `oxide.config.ts` — the config path itself never
  evaluates JavaScript.
- **One wire format.** ODF, the Oxide Data Format, is a devalue-compatible superset encoded
  natively in Rust and decoded — and now re-encoded — by a single-file, zero-dependency
  TypeScript module. The same bytes go to the cache and to the wire: `'use cache'` values are
  stored ODF-encoded, so `Date`, `Map`, `Set` and `BigInt` survive a cache round trip, on the
  Node and V8 backends alike. Sixteen golden fixtures lock the format byte-exactly on both
  sides, and eight encoder goldens lock the client-side `Date` / `BigInt` / `Map` / `Set` /
  `URL` / `RegExp` / typed-array layout against the Rust encoder byte for byte.
- **Content-addressed to the byte.** A demand-driven incremental graph with early cutoff, a
  `blake3` content-addressed store on disk (`redb` index, `zstd` blobs), byte-stable manifests
  and typegen, and a three-tier cache — L0 in-process response cache, L1 data cache, L2
  pluggable remote handler — with tags, stale-while-revalidate, single-flight and build seeding.
- **A single binary.** A `hyper` 1.x + `rustls` server (HTTP/1 + HTTP/2, with HTTP/3 over
  QUIC and automatic `Alt-Svc` advertisement on the TLS listener) with precompressed
  static serving (zstd › brotli › gzip), range requests, streaming responses with dynamic
  compression and `Server-Timing`. Everything that must run JavaScript does so behind one
  `JsHost` trait: a `deno_core` V8 isolate pool with build-time startup snapshots, or a Node
  child-process pool — both speaking the same op protocol.
- **Deploy anywhere.** Twelve deploy targets — Binary, Node, Static, Docker, Cloudflare,
  Cloudflare Pages, Lambda, Vercel, Vercel Functions, Netlify, Deno, Bun — behind a capability
  matrix that refuses at build time a configuration the target cannot honor.
- **Security and observability built in.** CSP and CSRF modes, SSRF allow-lists, trusted
  proxies, token-bucket rate limits, encrypted session cookies (ChaCha20-Poly1305 AEAD with
  key rotation, decoded into `locals.session`), ODF decode hardening with
  prototype-pollution guards. Prometheus-format metrics, W3C trace-context propagation into
  outgoing fetches, stage spans and opt-in OTLP export.
- **Platform services included.** Service-worker auto-registration with a fixed cache
  strategy, i18n content negotiation, signed image-transform URLs, and an Open Graph image
  endpoint — `/_oxide/og` renders title/subtitle cards to PNG through a swappable backend.
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

cargo build --release -p oxide-cli
target/release/oxide-cli doctor
```

Run the smoke app end to end:

```sh
cd examples/smoke-app-react

../../target/release/oxide-cli routes   # scan routes, emit typegen
../../target/release/oxide-cli build    # bundle, prerender, emit deploy artifacts
../../target/release/oxide-cli start    # serve /, /blog/:slug, /_oxide/health
```

Or scaffold a fresh app — 4 frameworks × minimal / blog / dashboard / storefront
(16 templates, each shipping a streaming `defer()` demo):

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
> `oxide schema` prints a JSON Schema for `oxide.config.ts` — reference it via `$schema` for
> editor completions. End-to-end browser tests need a Chromium install:
> `pnpm test:playwright:install && pnpm test:playwright`.

## The CLI

One binary, twenty subcommands:

| | Command | What it does |
| --- | --- | --- |
| **Core loop** | `dev` | Dev server + watcher + HMR hub + inspector; per-file invalidation, idle pre-warm; `--https --qr --open --proxy` |
| | `build` | Client+SSR bundles, prerender, typegen, `rkyv` route manifest, V8 snapshot, L0 cache seed, deploy artifacts, forbidden-import guard |
| | `start` · `preview` | Production server (pipeline below); health at `/_oxide/health` |
| | `render <route>` | Dispatch a page render, HTML to stdout (`--url --param --data --stream`) |
| **Introspection** | `routes` | Scan routes, infer render modes, emit typegen, print the route table |
| | `analyze` | Per-route JS bytes vs `config.budgets` — exits non-zero on breach; top modules by reachable bytes |
| | `check` | Config + route diagnostics; typechecks (tsgo) and lints (oxlint) when on PATH — loud `SKIPPED` otherwise |
| | `why <module>` | Which routes import a module, which chunks contain it |
| | `doctor` | 7 environment checks: node, cargo, config, routes dir, cache dir, secrets, cookie keys |
| **Authoring** | `generate` | Scaffold route / layout / action files per framework |
| | `add <module>` | Register a module in `.oxide/modules.json` |
| | `schema` | JSON Schema for `oxide.config.{ts,json}` |
| | `config` | Load the resolved config, print the framework adapter descriptor |
| | `migrate <source>` | Migration codemods for Next, Nuxt, SvelteKit, Vite (`--write` to apply on disk) |
| **Operations** | `secrets rotate` | Drop the build secret; the next build mints a fresh one |
| | `cache inspect\|clear\|stats` | Inspect, clear or measure `.oxide/cache` |
| | `audit` | Static advisory pass over registered modules and plugins |
| | `test` | Build-manifest gate for the app test helpers |
| | `mcp` | stdio MCP server exposing `oxide_routes`, `oxide_cache`, `oxide_graph`, `oxide_errors` |

## Architecture

The request pipeline is ordered so the hot path completes **without entering JavaScript**
whenever the answer is cached, redirected, rewritten or static:

```text
request → hyper 1.x → limits + request id → static assets (precompressed, hashed, range, ETag)
        → native rules (redirects · rewrites · headers · basic auth · rate limits)
        → route match (rkyv archive, memory-mapped)
        → L0 response cache (weak-ETag 304s · stale-while-revalidate · single-flight · build-seeded)
        → JS middleware (runs before loaders/actions, injects event.locals, may short-circuit or rewrite)
        → loaders: layout chain outer→inner (await parent()), then the page load — the last
          layout and the page load run in parallel when the page never calls parent();
          deferred values resolve and reach the client as a nonce'd script
        → dispatch: render | action | endpoint (route.ts) | server fn
        → streaming response (real SSR chunks) + Server-Timing + dynamic compression
        → cache fill + tag invalidation
```

```text
crates/         cli · config · graph · compiler · bundler · jshost · router · server ·
                render · cache · odf · assets · dev · plugin · adapters · core-wasm ·
                diagnostics                                   (17 crates, ~60k lines)
packages/       oxide (runtime) · @oxide/react · @oxide/vue · @oxide/svelte ·
                @oxide/solid · create-oxide                   (zero runtime deps)
conformance/    Atlas reference app — one assertion set × four adapters
examples/       smoke-app-{react,vue,svelte,solid} — end-to-end fixtures
benchmarks/     method, harness and the >5% CI regression gate
tests/          Playwright specs + fixture check scripts
docs/           architecture.md · Technical Specification v0.1
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
| React | `@oxide/react` | `.tsx` | native | refresh |
| Vue | `@oxide/vue` | `.vue` | core | component |
| Svelte | `@oxide/svelte` | `.svelte` | core | component |
| Solid | `@oxide/solid` | `.tsx` | native | component |

Rendering, hydration and HMR strategy are the only things an adapter owns. The rest of the
client runtime is core-owned (`packages/oxide`) and shipped identically to every framework:
the ODF decoder, the island loader (strategies `load · idle · visible · media(q)` on
`<ox-island>` hosts), progressive form `enhance`, script strategies, deferred-boundary glue,
navigation loading boundaries (`onNavigateStart/End` + per-adapter `renderLoading`),
manifest-driven chunk preloading on prefetch/goto, and a Core Web Vitals reporter that
beacons to `/_oxide/vitals` with zero third-party script. Island hydration calls the real
framework runtime on all four adapters (`createSSRApp`/`mount(hydrate)`/`hydrate`/`createRoot`).

## One wire format: ODF

ODF is a version-prefixed, type-tagged, little-endian binary format — a devalue-compatible
superset covering JSON primitives plus `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`,
typed arrays, `undefined`, cyclic and shared references, `Error` and deferred promise slots.

- Encoded natively in Rust (`oxide-odf`), decoded by a single-file zero-dependency TypeScript
  module with hardening parity: 256 MiB input cap, depth 512, 1M refs, `__proto__` /
  `constructor` rejection, stable `ODF-*` error codes.
- Sixteen golden fixtures are committed as bytes and asserted **byte-exact** by both the Rust
  encoder and the TypeScript decoder — the format cannot drift between sides.

## Testing

The repository is built test-first: each crate's `*_tdd.rs` suites were written against its
contract before the implementation.

- **1087 Rust test functions** across 17 crates (48 of them engine-dependent e2e tests gated
  behind `#[ignore]`), including `proptest` property suites on the router and graph engines
  and `cargo-fuzz` harnesses for the ODF decoder, router matcher, config extractor and path
  splitter.
- **342 TypeScript test cases** across 46 files on Node's built-in test runner — including
  the ODF decoder against the same golden fixtures as the Rust encoder (typed values:
  `BigInt`, `Date`, `Map`, `Set`, `URL`, `RegExp`, typed arrays), router / island
  tests against a hand-rolled fake DOM (no jsdom, no deps), suites for the defer script
  protocol and client-side loading boundaries, the per-adapter `<Image>` components,
  and the client message-catalog loader.
- **Conformance gate** — the Atlas reference app runs one 24-assertion set (contract shape,
  SSR content + framework markers, merged head, deterministic markup, streaming boundaries,
  deferred boundaries in settle order, error / not-found, islands, actions JS-on/off, server
  functions, cache shapes, client router, prefetch) against all four adapters: 96 checks per
  release, and 21 Rust snapshot tests lock the emitted HTML and deploy artifacts byte-for-byte.
- **Playwright** — 17 Chromium specs across 7 files that boot the real binary and drive a
  real browser: SSR identity with zero console errors, island hydration (click 3 → 4 → 5),
  navigation loading boundaries, middleware locals + redirect, socket round-trip, ordered
  SSE streaming, and the config-gated `/_oxide/og` endpoint (PNG magic bytes, 400 on a
  missing title, byte-determinism).

CI runs five gates: Rust fmt + tests on a Linux / macOS / Windows matrix, Node 24 package
tests, the conformance suite, the V8 fixture check, and a benchmark gate that **fails the
build on >5% regression** against `benchmarks/report.md`.

## Project status

> [!WARNING]
> Oxide is pre-alpha software under active development toward
> [spec v0.1](docs/Oxide%20Technical%20Specification.md). The adapter contract is
> versioned and treated as frozen; everything else may churn.

**Shipped and gated today** — the production pipeline (hyper + rustls HTTP/1+2 with HTTP/3
discovery via `Alt-Svc`, native rules, L0 response cache with configurable Vary, streaming
SSR as the primary live path on both backends), `oxide build` with prerender that runs full
layout chains (`await parent()` threading, last layout + page load in parallel when the page
doesn't call `parent()`, real error statuses seeded into the cache), snapshots and 12 deploy
targets, real framework SSR and real island hydration on all four adapters, client navigation
that resolves deferred values on all four adapters, the ODF wire format with byte-parity
encoders on both sides (including ODF-encoded `'use cache'` on Node and V8), six-layer config
extraction, cache semantics with tags / single-flight / Vary, full outer→inner middleware
ancestor chains with threaded `event.locals` whose headers ride cached responses too,
`route.ts` endpoint dispatch, socket routes (RFC6455 handshake through `socket:run`),
connected dev HMR including the loader-edit round trip and a one-time client bundle on
fresh dev, `/_oxide/og` config-gated Open Graph PNG rendering (SVG → resvg → PNG with an
MP area cap), module packages (`oxide.module`) that inject routes under `/modules/<name>`
and dispatch **all 19 plugin hooks** through the real bundler and pipeline (build-time
effects of six hooks gated by the fixture CI job), per-adapter `<Image>` components over
the signed AVIF/WebP/JPEG/PNG pipeline (build-time `srcset` + intrinsic dimensions,
runtime `/_oxide/image` with `remotePatterns` + SSRF guards), per-locale message catalogs
emitted at build with a client-side loader, `content.d.ts` typed-content emission, real
`oxide add` (package-manager-detected installs), `oxide audit` (live OSV queries with a
24h cache) and `oxide migrate` with working `--write` codemods, per-route prerender reuse
backed by the persisted DemandGraph (`.oxide/graph.json`, surfaced in `analyze` and `mcp`),
analyze's top-modules-by-reachable-bytes report, parse-error diagnostics with line/column
and persisted code frames, zero-copy L0 cache hits over `Arc` entries with MAC-sealed
Redis/S3 payloads, a moka asset-body cache, build-time font subsetting, locale-aware
`href()` typegen, constant-time Basic-auth checks and a middleware rewrite-loop guard
(508), `oxide check` with tsgo + oxlint, and the introspection tooling (`doctor`, `why`,
`analyze`, `schema`, `mcp`).

**Built, wiring in progress** — streaming deferred values (today the worker awaits every
`defer()` before the response flushes, so one slow `defer()` stalls TTFB), partial
prerendering (the shell splitter and assembler are tested; no render path constructs shells
yet), session-cookie *writing* (the decode path is live, `Set-Cookie` is not), HTTP/3
request bodies and streamed responses (HTTP/1+2 unaffected), the edge build
(`dist/edge.wasm` emission is dormant and the edge handler does not call ODF decode yet),
socket fan-out / presence (sends reach only the calling socket), the *last mile* of the
plugin hook surface (`handleHotUpdate` fires on the once-probe path only, 8 hooks receive
arguments via a positional fallback table, module `routes` / `serverMiddleware` pools are
passed only at serve boot, and `cacheHandler` is consulted in dev+build but not in
`start`), plugin transforms on the V8 render path (the compile op exists; V8 bypasses it),
client-side i18n hydration (the server resolves the locale but never emits `<html lang>`,
so the client catalog loader always falls back to `'en'`; ICU formatting is not yet
evaluated at render time), image-result caching and concurrency limits (every request
re-transforms; `sizes` is user-supplied and the LQIP placeholder arm is unreachable), the
collections runtime query surface (`content.d.ts` emits inferred types; declared schemas
and typed queries are not wired), route `meta` exports (title path wired; the flat shape
drops richer head fields), strict MAC enforcement for L2 cache entries (a legacy-JSON
fallback still accepts unsealed payloads), single-flight cleanup when a follower subscribes
before the owner short-circuits, MDX compilation beyond the reachable compile arm, and the
neutral IR renderer's layout nesting (dev-fixture path only). See
[`HANDOFF.md`](HANDOFF.md) for the current thread list and what remains.

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — spec § → crate map, pipeline and
  cross-cutting invariants
- [`docs/Oxide Technical Specification.md`](docs/Oxide%20Technical%20Specification.md)
  — the full design document: Part I is the v0.1 spec (20 decisions, 26 sections); Part II
  is the v0.2 implementation edition (work packages, waves, appendices)
- [`docs/plugin-compat.md`](docs/plugin-compat.md) — plugin kinds × hooks compatibility
  matrix (what is wired, partial, or unsupported per plugin kind)
- [`benchmarks/README.md`](benchmarks/README.md) — benchmark method and per-route JS budgets
- [`conformance/`](conformance/) — the Atlas reference app and the cross-adapter suite
- [`examples/smoke-app-react`](examples/smoke-app-react) — the minimal app used in the
  end-to-end smoke run
