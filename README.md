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
    <img src="https://img.shields.io/badge/Code-%E2%89%8878k%20lines-9CF?style=flat-square" alt="Lines of code"/>
    <img src="https://img.shields.io/badge/Tests-944%20Rust%20%7C%20212%20TS-brightgreen?style=flat-square" alt="Tests"/>
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
  hydrate, head and plugins hooks. Each adapter ships a zero-dependency neutral IR renderer
  *and* an opt-in bridge to the real upstream renderer (`react-dom/server`,
  `@vue/server-renderer`, `svelte/server`, `@solidjs/web`). Framework libraries are optional
  peer dependencies: when they are absent, the adapter degrades gracefully to the IR renderer.
- **Fast paths that skip JavaScript.** The route table is a build-time radix tree serialized
  to an `rkyv` archive and memory-mapped at boot — matches read the mapped bytes in place.
  Redirects, rewrites, headers, basic auth and rate limits are evaluated natively in Rust.
  Typed config is statically extracted from `oxide.config.ts` — the config path itself never
  evaluates JavaScript.
- **One wire format.** ODF, the Oxide Data Format, is a devalue-compatible superset encoded
  natively in Rust and decoded by a single-file, zero-dependency TypeScript module. The same
  bytes go to the cache and to the wire. Sixteen golden fixtures lock the format byte-exactly
  on both sides.
- **Content-addressed to the byte.** A demand-driven incremental graph with early cutoff, a
  `blake3` content-addressed store on disk (`redb` index, `zstd` blobs), byte-stable manifests
  and typegen, and a three-tier cache — L0 in-process response cache, L1 data cache, L2
  pluggable remote handler — with tags, stale-while-revalidate, single-flight and build seeding.
- **A single binary.** A `hyper` 1.x + `rustls` server (HTTP/1 + HTTP/2) with precompressed
  static serving (zstd › brotli › gzip), range requests, streaming responses with dynamic
  compression and `Server-Timing`. Everything that must run JavaScript does so behind one
  `JsHost` trait: a `deno_core` V8 isolate pool with build-time startup snapshots, or a Node
  child-process pool — both speaking the same op protocol.
- **Deploy anywhere.** Twelve deploy targets — Binary, Node, Static, Docker, Cloudflare,
  Cloudflare Pages, Lambda, Vercel, Vercel Functions, Netlify, Deno, Bun — behind a capability
  matrix that refuses at build time a configuration the target cannot honor.
- **Security and observability built in.** CSP and CSRF modes, SSRF allow-lists, trusted
  proxies, token-bucket rate limits, ChaCha20-Poly1305 AEAD session cookies with key rotation,
  ODF decode hardening with prototype-pollution guards. Prometheus-format metrics, W3C trace
  context propagation, stage spans and opt-in OTLP export.
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
| | `analyze` | Per-route JS bytes vs `config.budgets` — exits non-zero on breach |
| | `check` | Config load + route scan diagnostics |
| | `why <module>` | Which routes import a module, which chunks contain it |
| | `doctor` | 7 environment checks: node, cargo, config, routes dir, cache dir, secrets, cookie keys |
| **Authoring** | `generate` | Scaffold route / layout / action files per framework |
| | `add <module>` | Register a module in `.oxide/modules.json` |
| | `schema` | JSON Schema for `oxide.config.{ts,json}` |
| | `config` | Load the resolved config, print the framework adapter descriptor |
| | `migrate <source>` | Dry-run migration plan for Next, Nuxt, SvelteKit, Vite |
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
        → JS middleware (only when present)
        → dispatch: render | action | server fn
        → streaming response + Server-Timing + dynamic compression
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
`<ox-island>` hosts), progressive form `enhance`, script strategies, deferred-boundary glue
and a Core Web Vitals reporter that beacons to `/_oxide/vitals` with zero third-party script.

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

- **944 Rust test functions** across 17 crates (43 of them engine-dependent e2e tests gated
  behind `#[ignore]`), including `proptest` property suites on the router and graph engines.
- **212 TypeScript test cases** across 7 suites on Node's built-in test runner — including
  the ODF decoder against the same golden fixtures as the Rust encoder, and router / island
  tests against a hand-rolled fake DOM (no jsdom, no deps).
- **Conformance gate** — the Atlas reference app runs one 22-assertion set (contract shape,
  SSR content + framework markers, merged head, deterministic markup, streaming boundaries,
  error / not-found, islands, actions JS-on/off, server functions, cache shapes, client
  router) against all four adapters: 88 checks per release.
- **Playwright** — Chromium specs that boot the real binary and click an island 3 → 4 → 5,
  asserting zero console errors.

CI runs five gates: Rust fmt + tests on a Linux / macOS / Windows matrix, Node 24 package
tests, the conformance suite, the V8 fixture check, and a benchmark gate that **fails the
build on >5% regression** against `benchmarks/report.md`.

## Project status

> [!WARNING]
> Oxide is pre-alpha software under active development toward
> [spec v0.1](docs/Oxide%20Technical%20Specification%20v0.1.md). The adapter contract is
> versioned and treated as frozen; everything else may churn.

**Shipped and gated today** — the production pipeline (hyper + rustls, rules, L0 cache,
streaming), `oxide build` with prerender / snapshots / 12 deploy targets, real framework SSR
on all four adapters, the ODF wire format, six-layer config extraction, cache semantics with
tags and single-flight, the security stack, metrics and tracing, and the introspection
tooling (`doctor`, `why`, `analyze`, `schema`, `mcp`).

**Built, wiring in progress** — client router boot and client-side navigation (`/__data`),
nested layout chains, `loading` / `error` / `not-found` boundary dispatch, endpoint routes
and middleware execution, deferred-value streaming end to end, non-React island hydration,
HMR browser push, font subsetting and partial prerendering (PPR) assembly. See
[`HANDOFF.md`](HANDOFF.md) for the current thread list.

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — spec § → crate map, pipeline and
  cross-cutting invariants
- [`docs/Oxide Technical Specification v0.1.md`](docs/Oxide%20Technical%20Specification%20v0.1.md)
  — the full design document: 20 decisions, 26 sections, roadmap
- [`benchmarks/README.md`](benchmarks/README.md) — benchmark method and per-route JS budgets
- [`conformance/`](conformance/) — the Atlas reference app and the cross-adapter suite
- [`examples/smoke-app-react`](examples/smoke-app-react) — the minimal app used in the
  end-to-end smoke run
