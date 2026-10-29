<div align="center">
  <img src="docs/logo.png" alt="Oxide" width="180"/>
  <p>
    <strong>One Rust core. Every framework.</strong><br/>
    A framework-agnostic full-stack meta-framework with a <strong>Rust core</strong>. React, Vue,
    Svelte and Solid are first-class adapters over one shared engine that owns routing, data,
    caching, bundling, serving and deployment — so anything framework-neutral behaves
    identically on all four. Built against the <em><a href="docs/spec-v0.1.md">Oxide Technical Specification v0.1</a></em>.
  </p>
  <p>
    <a href="#install"><img src="https://img.shields.io/badge/Rust-stable%20%7C%20edition%202024-000000?style=flat-square&logo=rust" alt="Rust"/></a>
    <a href="https://github.com/elcoosp/oxide/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-blue?style=flat-square" alt="License"/></a>
    <a href="https://github.com/elcoosp/oxide/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/Platforms-Linux%20%7C%20macOS%20%7C%20Windows-9D4EDD?style=flat-square" alt="Platforms"/></a>
    <a href="#install"><img src="https://img.shields.io/badge/Node-%E2%89%A524-339933?style=flat-square&logo=node.js" alt="Node"/></a>
    <a href="#framework-adapters"><img src="https://img.shields.io/badge/Adapters-React%20%7C%20Vue%20%7C%20Svelte%20%7C%20Solid-007ACC?style=flat-square" alt="Adapters"/></a>
    <a href="https://github.com/elcoosp/oxide"><img src="https://img.shields.io/badge/Workspace-18%20crates%20%2B%209%20packages-6F4E37?style=flat-square" alt="Workspace"/></a>
    <a href="https://github.com/elcoosp/oxide"><img src="https://img.shields.io/badge/Code-%E2%89%88113k%20lines-9CF?style=flat-square" alt="Lines of code"/></a>
    <a href="https://github.com/elcoosp/oxide/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/Tests-1239%20Rust%20%7C%20445%2F445%20TS-brightgreen?style=flat-square" alt="Tests"/></a>
    <a href="#one-wire-format-odf"><img src="https://img.shields.io/badge/ODF-16%20golden%20fixtures-00BFFF?style=flat-square" alt="ODF golden fixtures"/></a>
    <a href="#features"><img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero dependencies"/></a>
  </p>
  <p>
    <strong><a href="#features">Features</a> · <a href="#install">Quick start</a> · <a href="#the-cli">The CLI</a> · <a href="#architecture">Architecture</a> · <a href="#framework-adapters">Adapters</a> · <a href="#one-wire-format-odf">ODF</a> · <a href="#testing">Testing</a> · <a href="#project-status">Status</a></strong>
  </p>
</div>

## What is Oxide?

A **framework-agnostic, full-stack meta-framework** with a Rust core. React, Vue, Svelte and Solid are first-class adapters over one shared engine that owns routing, data, caching, bundling, serving and deployment. A feature that can be framework-neutral lives in the core, so it behaves identically on all four.

The pitch is a single Rust binary that:
- compiles and serves four different frontend frameworks through one server
- caches at three tiers (in-process, on-disk, remote) with one wire format
- ships to 12 deploy targets behind a build-time capability matrix
- refuses to silently degrade — every stub is documented in [`docs/deviations.md`](docs/deviations.md)

## Features

- **One engine, four frameworks.** A versioned adapter contract (`contract: 1`) defines a capabilities manifest — suspense mode, islands support, HMR strategy — plus render, hydrate, head, loading and plugins hooks. Each adapter ships a zero-dependency neutral IR renderer *and* a bridge to the real upstream renderer (`react-dom/server`, `@vue/server-renderer`, `svelte/server`, `@solidjs/web`). Framework libraries are optional peer dependencies: when absent, the adapter degrades to the IR renderer. **Real streaming SSR is the factory default.**
- **Fast paths that skip JavaScript.** The route table is a build-time compiled route tree. Redirects, rewrites, headers, basic auth and rate limits are evaluated natively in Rust. Typed config is statically extracted from `oxide.config.ts` — the config path itself never evaluates JavaScript.
- **One wire format.** ODF, the Oxide Data Format, is a devalue-compatible superset encoded natively in Rust and decoded — and re-encoded — by a single-file, zero-dependency TypeScript module. `'use cache'` values are stored ODF-encoded, so `Date`, `Map`, `Set` and `BigInt` survive a cache round trip.
- **Content-addressed to the byte.** A demand-driven incremental graph with early cutoff, a `blake3` content-addressed store on disk (`redb` index, `zstd` blobs), byte-stable manifests and typegen, and a three-tier cache — L0 in-process, L1 data cache, L2 pluggable remote handler — with tags, stale-while-revalidate, single-flight and MAC-sealed remote payloads on Redis, S3 and HTTP.
- **A single binary.** A `hyper` 1.x + `rustls` server (HTTP/1 + HTTP/2, with HTTP/3 over QUIC and automatic `Alt-Svc`), precompressed static serving (zstd → brotli → gzip), range requests, streaming responses with dynamic compression and `Server-Timing`, zero-downtime reload via `SO_REUSEPORT`, and §5.6 dev cancellation. Everything that must run JavaScript does so behind one `JsHost` trait: a `deno_core` V8 isolate pool with build-time startup snapshots, or a Node child-process pool — both speaking the same op protocol.
- **Deploy anywhere.** Binary, Node, Static, Docker, Cloudflare, Cloudflare Pages, Lambda, Vercel, Vercel Functions, Netlify, Deno, Bun — behind a capability matrix that refuses at build time a configuration the target cannot honor.
- **Security and observability built in.** CSP and CSRF modes, SSRF allow-lists, trusted proxies, token-bucket rate limits, encrypted session cookies (ChaCha20-Poly1305 AEAD with key rotation), ODF decode hardening with prototype-pollution guards. Prometheus-format metrics, W3C trace-context propagation into outgoing fetches, actions and endpoints, stage spans and opt-in OTLP export.
- **Conformance-gated.** One reference app (Atlas), one assertion set, all four adapters. A release is blocked the moment any adapter drifts.

## Install

Prerequisites: a **Rust stable** toolchain (edition 2024) and **Node ≥ 24**. The
TypeScript packages have zero runtime npm dependencies and run on Node's native
type stripping — no transpile step.

```bash
# The CLI — a single static binary.
cargo install oxide-fw-cli

# Sanity-check the environment (node, cargo, config, cache dir, build secret).
oxide doctor

# Scaffold a project.
pnpm create oxide my-app --framework react --template blog
# or: npx create-oxide@latest my-app --framework react --template blog

cd my-app
oxide dev          # dev server with HMR
oxide build        # production build
oxide start --port 3000
```

`create-oxide` is a separate npm package
([npmjs.com/package/create-oxide](https://www.npmjs.com/package/create-oxide));
it never ships inside the `oxide` binary, and both invocations above call the
same scaffolder.

> [!NOTE]
> **v0.1.0 is not yet published.** Until `cargo install oxide-fw-cli` resolves
> on crates.io, build the binary from source — see
> [Building from source](#building-from-source). Every command above works the
> same against `./target/release/oxide`.

## Building from source

Contributors and anyone testing the pre-release:

```bash
git clone https://github.com/elcoosp/oxide
cd oxide
cargo build --release -p oxide-fw-cli

# Sanity-check the environment.
./target/release/oxide doctor
```

The repo ships four end-to-end fixtures under
[`examples/smoke-app-{react,vue,svelte,solid}`](examples). `oxide build --root
examples/smoke-app-react` followed by `oxide start --root
examples/smoke-app-react --port 3100` is the fastest path to a running app that
exercises every feature.

## The CLI

One binary, one entry point:

| Command | What it does |
| --- | --- |
| `oxide dev` | Dev server: on-demand ESM, HMR, structured logging |
| `oxide build` | Production build (all environments + manifests) |
| `oxide start` | Production server (binary target) |
| `oxide preview` | Preview the production build locally |
| `oxide routes` | Print the route table with inferred modes and cache policy |
| `oxide analyze` | Bundle and budget report |
| `oxide check` | Typecheck (tsgo), lint (oxlint), config + route diagnostics |
| `oxide why` | Explain importers and chunks of a module |
| `oxide doctor` | Environment and config health |
| `oxide secrets` | Rotate the per-project build secret |
| `oxide add` | Install a module |
| `oxide generate` | Scaffold route, layout or action files |
| `oxide cache` | Cache inspection |
| `oxide audit` | Plugin and dependency advisories |
| `oxide migrate` | Migrate from Next / Nuxt / SvelteKit / Vite |
| `oxide test` | In-process test helper: boot the app and expose `fetch` |
| `oxide mcp` | Model Context Protocol server (route/config/audit tools for agents) |
| `oxide schema` | Print the typed config JSON Schema |
| `oxide config` | Print the resolved config as JSON (for debugging layer merging) |
| `oxide render` | Render one route to stdout (debug the SSR output in isolation) |

## Architecture

```
                    ┌─────────────────────────────────────┐
                    │  oxide (single Rust binary)         │
                    ├─────────────────────────────────────┤
    HTTP request ──▶│  server  ·  router  ·  cache        │
                    │  render  ·  jshost  ·  plugin       │
                    │  graph   ·  assets  ·  odf          │
                    └───────────────┬─────────────────────┘
                                    │ JsHost trait
                                    ▼
                    ┌─────────────────────────────────────┐
                    │  deno_core V8 isolate  │  Node pool │
                    └───────────────┬─────────────────────┘
                                    │ adapter contract v1
                                    ▼
                    ┌─────────────────────────────────────┐
                    │ React │ Vue │ Svelte │ Solid        │
                    └─────────────────────────────────────┘
```

The Rust core never trusts JavaScript with a task it can do faster itself: routing, caching, headers, rate limits, and static serving are Rust. JavaScript only runs when a framework component or user loader needs it, and every such call goes through the same `JsHost` trait with a uniform op protocol — an in-process V8 pool for the binary target, a child-process Node pool for Node-serverless.

`crates/` holds the 18 core crates. `packages/` holds the 9 TypeScript packages (the four adapter bridges, the core runtime, the CLI scaffolder, the two website/OG utilities, the Tailwind plugin).

## Framework adapters

All four adapters share one contract (`contract: 1`) and one conformance suite. Each is:

- a **renderer** — real streaming SSR against the framework's own server runtime, with an IR (neutral element tree) fallback when the framework is not installed
- a **hydrator** — real client-side hydration against the framework's client runtime, with a deterministic IR fallback
- a **head manager** — merges `<title>`, `<meta>`, `<link>` across layouts and page
- a **router adapter** — receives `update(ClientInput)` on navigation and re-renders into the framework's root
- a **plugin bridge** — exposes the framework's conventions (file extensions, JSX transforms) to the build

| Adapter | Real SSR | Real hydration | HMR | Islands |
| --- | --- | --- | --- | --- |
| React | `react-dom/server` | `react-dom/client` | ✓ | ✓ |
| Vue | `@vue/server-renderer` | `createSSRApp` | ✓ | ✓ |
| Svelte | `svelte/server` | `hydrate()` | ✓ | ✓ |
| Solid | `@solidjs/web` | `hydrate()` | ✓ | ✓ |

Framework libraries are **optional peer dependencies** — when missing, the adapter stays on the IR renderer. That is what lets a Svelte app run on a machine that only has React installed (and vice versa).

## One wire format: ODF

ODF (Oxide Data Format) is the only format that crosses the Rust ↔ TypeScript boundary. The same bytes go to the wire and into the cache:

- `Date`, `Map`, `Set`, `BigInt`, `URL`, `RegExp`, `undefined`, typed arrays, and `NaN`/`Infinity` survive a full round trip
- the encoder is native Rust (`crates/odf`), the decoder is a single-file zero-dependency TypeScript module (`packages/core/src/client/odf-decode.ts`)
- 16 golden fixtures and 8 encoder goldens lock the format byte-exactly on both sides
- `'use cache'` values are stored ODF-encoded, so a cached `Date` comes back as a `Date`, not a string

## Testing

| Suite | Count | How to run |
| --- | --- | --- |
| Rust | 1187 pass in a default `cargo test --workspace` run (1239 test fns total; the rest `#[ignore]`d for V8/network/snapshot and run by dedicated CI jobs) | `cargo test --workspace --no-fail-fast` |
| Framework adapters (Node) | 212 core + 67 react + 55 vue + 55 svelte + 50 solid | `node --test packages/*/test/*.test.ts` |
| Playwright (real browser, 3 engines) | 26 scenarios × 3 engines | `cd tests/playwright && pnpm exec playwright test` |
| Benchmarks | deterministic byte counts + wall-clock info | `./benchmarks/run.sh`, gated by `benchmarks/gate.sh` |

Every Rust crate and TypeScript package is CI-tested on Linux, macOS **and Windows**. The workspace is clippy-clean at `-D warnings` with **zero `#[allow(clippy::*)]` attributes**.

Run a single crate: `cargo test -p oxide-fw-<name>`. Run a single adapter: `node --test packages/<adapter>/test/*.test.ts`.

## Project status

Oxide is **v0.1 release-ready**. CI enforces 22 jobs across Linux, macOS and Windows — 21 run on every push, plus a scheduled Miri job (nightly, not a push gate):

- `rust (ubuntu / macos / windows)` — fmt + clippy + 1239 tests
- `node` — all framework packages
- `playwright × 3` — Chromium, Firefox, WebKit
- `bench-gate` — deterministic byte-count regression, ≥5 % threshold
- `HMR latency` — edit-to-200 within budget
- `launch-content`, `spec-anchor`, `doc-rot` — staleness + spec coverage gates
- `conformance × 3` — reference app parity across all four adapters
- `plugin-fixture`, `wasm32-edge`, `V8 backend`, `loader-e2e`, `Redis`, `ODF-fuzz` — subsystem gates

### Known gaps

- **`suspense: 'core'` boundaries** carry rendered HTML across `ox:swap`, not the structured value. This is by design for Suspense (the whole point is "swap in the ready subtree") but is distinct from **`defer()` in loader data**, which *does* carry structured ODF values — see `packages/core/src/client/defer-glue.ts` and the `/deferred` route in the smoke apps.
- **Miri** is skipped in CI. Enabling it would require `#[cfg(miri)]` guards on the crates that assume a real allocator / OS; not worth it for v0.1.
- **[`docs/deviations.md`](docs/deviations.md)** carries 1196 item-level `///` docs deferred to v0.2.

### What's next for v0.1

- crates.io + npm publish (manual first-publish, then OIDC)
- `release.yml` automation on tag

## Documentation

- **[`docs/spec-v0.1.md`](docs/spec-v0.1.md)** — the specification this release implements
- **[`docs/spec-v0.2-draft.md`](docs/spec-v0.2-draft.md)** — the working draft for the next release
- **[`docs/deviations.md`](docs/deviations.md)** — every known gap between spec and code, item by item
- **[`docs/architecture.md`](docs/architecture.md)** — how the crates fit together
- **[`docs/plugin-compat.md`](docs/plugin-compat.md)** — plugin API surface and Vite/Rollup compatibility
- **[`benchmarks/report.md`](benchmarks/report.md)** — the current perf table with the deterministic-vs-informational split documented

---

<div align="center">
<sub>Built against the Oxide Technical Specification v0.1 · MIT OR Apache-2.0</sub>
</div>
