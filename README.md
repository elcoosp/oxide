<div align="center">
  <img src="docs/logo.png" alt="Oxide Logo" width="200"/>
  <p>
    <strong>One Rust core. Every framework.</strong><br/>
    A framework-agnostic, full-stack meta-framework with a <strong>Rust core</strong>. React, Vue,
    Svelte and Solid are first-class adapters over one shared engine that owns routing, data,
    caching, bundling, serving and deployment — so a feature that can be framework-neutral
    behaves identically on all four. Implemented end-to-end from the
    <em>Oxide Technical Specification v0.1</em>.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-stable%20%7C%20edition%202024-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-blue?style=flat-square" alt="License"/>
    <img src="https://img.shields.io/badge/Platforms-Linux%20%7C%20macOS%20%7C%20Windows-9D4EDD?style=flat-square" alt="Platforms"/>
    <img src="https://img.shields.io/badge/Adapters-React%20%7C%20Vue%20%7C%20Svelte%20%7C%20Solid-007ACC?style=flat-square" alt="Adapters"/>
    <img src="https://img.shields.io/badge/Workspace-17%20crates%20%2B%206%20packages-6F4E37?style=flat-square" alt="Workspace"/>
    <img src="https://img.shields.io/badge/Code-41k%20LOC-9CF?style=flat-square" alt="Lines of code"/>
    <img src="https://img.shields.io/badge/Tests-608%20passing-brightgreen?style=flat-square" alt="Tests"/>
    <img src="https://img.shields.io/badge/ODF-16%20golden%20fixtures-00BFFF?style=flat-square" alt="ODF golden fixtures"/>
    <img src="https://img.shields.io/badge/TS%20Runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero dependencies"/>
    <img src="https://img.shields.io/badge/Testing-TDD%20%7C%20proptest%20%7C%20golden-blueviolet?style=flat-square" alt="Testing"/>
  </p>
</div>

---

## Why Oxide

- **One engine, four frameworks.** Routing, data loading and actions, streaming SSR with
  deferred boundaries, islands, caching and HMR all live in the core. Adapters contribute
  only what is genuinely framework-specific: rendering, hydration and HMR strategy.
- **Fast paths that skip JS.** The route table is a build-time radix tree loaded zero-copy
  from an `rkyv` archive, declarative rules (redirects, rewrites, headers) are evaluated
  natively in Rust, and TypeScript config is statically extracted — nothing executes JS on
  the boot path.
- **Content-addressed to the byte.** A demand-driven incremental graph with early cutoff,
  a blake3 content-addressed store on disk (`redb` + `zstd`), byte-stable manifests and
  typegen, and a three-tier cache (L0 in-process / L1 disk / L2 remote) with tags,
  stale-while-revalidate and single-flight.
- **One wire format.** ODF — a devalue-compatible format encoded natively in Rust and
  decoded by a ~1 KB JavaScript module. Sixteen golden fixtures lock the format, read
  byte-exactly by both sides.
- **A single binary.** A `hyper` 1.x + `rustls` server, pure-Rust CSS, image and font
  pipelines, and deploy adapters for ten targets — from static hosting and containers to
  Node servers and edge runtimes.
- **Conformance-gated.** One reference app (Atlas), one assertion set, all four adapters.
  A release is blocked the moment any adapter drifts.

## Quick start

> [!NOTE]
> Prerequisites: a **Rust stable** toolchain and **Node ≥ 24**. The TypeScript packages have
> **zero npm dependencies** and run on Node's native type stripping — no transpile step.

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
../../target/release/oxide-cli build    # bundle, emit deploy artifacts
../../target/release/oxide-cli start    # serve /, /blog/:slug, /_oxide/health
```

Or scaffold a fresh app from a starter — 4 frameworks × minimal / blog / dashboard /
storefront:

```sh
node packages/create-oxide/src/cli.ts blog svelte my-app
```

Run the TypeScript test suites:

```sh
for d in packages/oxide packages/react packages/vue packages/svelte packages/solid \
         packages/create-oxide conformance; do (cd "$d" && npm test); done
```

## One binary, every command

```text
oxide-cli  dev · build · start · preview · routes · analyze · check · why ·
           doctor · add · generate · cache · audit · migrate · test
```

`analyze` also enforces per-route JS budgets from `config.budgets` and exits non-zero on
breach — the same numbers the CI regression gate watches.

## Architecture

The request pipeline is ordered so the hot path completes **without entering JavaScript**
whenever the answer is cached, redirected, rewritten or static:

```text
request → hyper (oxide-server) → limits / host checks → static assets →
native rules (oxide-router) → L0 cache → JS middleware (only when present) →
dispatch: render | action | endpoint | server fn →
streaming response + compression → cache fill + tags
```

```text
crates/        cli · config · graph · compiler · bundler · jshost · router · server ·
               render · cache · odf · assets · dev · plugin · adapters · core-wasm ·
               diagnostics                                   (17 crates)
packages/      oxide (runtime) · @oxide/react · @oxide/vue · @oxide/svelte ·
               @oxide/solid · create-oxide
conformance/   Atlas reference app — one assertion set × four adapters
benchmarks/    benchmark method and per-route budgets
examples/      smoke-app — minimal end-to-end app
docs/          architecture.md — spec § → crate map
```

Every external engine sits behind the isolation trait the spec mandates — `JsHost`,
`Bundler`, `Transformer`, CSS/image/font backends — so call sites never change when an
engine does.

## Framework adapters

| Adapter | Package | Owns |
| --- | --- | --- |
| React | `@oxide/react` | capabilities manifest, IR-based SSR, streaming boundary protocol, hydration, HMR strategy |
| Vue | `@oxide/vue` | identical contract — Vue renderer |
| Svelte | `@oxide/svelte` | identical contract — Svelte renderer |
| Solid | `@oxide/solid` | identical contract — Solid renderer |

The client router (~3 KB), form `enhance`, island loader and ODF decoder are **core-owned**
(`packages/oxide`) and shipped identically to every framework — navigation, prefetch, scroll
and islands behave the same everywhere.

## Testing

The repository is built test-first: each crate's tests were written against its contract
before the implementation (`*_tdd.rs` suites).

- **477 Rust tests** across 17 crates — unit, integration, and `proptest` suites on the
  router and graph engines.
- **131 TypeScript tests** across 7 packages — including the ODF decoder validated against
  the same golden fixtures as the Rust encoder, and router/island tests against a
  hand-rolled fake DOM (no jsdom, no deps).
- **Conformance gate** — the Atlas reference app runs one assertion set (SSR, streaming,
  boundaries, head merge, islands, actions on/off, server functions, caching shapes, client
  router contract) against all four adapters.

> [!TIP]
> `cargo test --workspace` and the per-package `npm test` are exactly what CI runs —
> Rust on a Linux / macOS / Windows matrix, Node 24 for the TypeScript packages.

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — spec § → crate map, pipeline and
  cross-cutting invariants
- [`conformance/`](conformance/) — the Atlas reference app and the cross-adapter suite
- [`benchmarks/README.md`](benchmarks/README.md) — benchmark method and per-route JS budgets
- [`examples/smoke-app-react`](examples/smoke-app-react) — the minimal app used in the end-to-end smoke run
