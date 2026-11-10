<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-adapters

**Deploy-target adapters: binary, node, static, docker, Cloudflare, serverless — with the capability matrix and the refusal rule.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-adapters?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-adapters) [![Downloads](https://img.shields.io/crates/d/oxide-fw-adapters?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-adapters) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-adapters?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-adapters) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-adapters?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-adapters?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-adapters)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

A deploy adapter transforms build output into a deployable artifact. The portable contract is the web-standard handler `(Request) => Promise<Response>` plus the manifests. A published capability matrix states per target which features work and how absent ones degrade; the build refuses a configuration that needs an unavailable feature rather than failing at runtime (§18).

## What it provides

- **`Target`** — the twelve deploy targets (§18 table) with `parse`/`as_str`
- **`Capability` / `Capability::ALL`** — the §18 capability set: the seven v0.1 rows, the V2-F05 `Live` row, and the v0.2 rows (V2-R02: `MixedIslands`, `Resume`, `Skew`, `DeltaDeploy`, `RustFns`, `Search`, `Offline`, `Brownout`, `Permissions`, `Attestation`)
- **`support(target, capability)`** — the matrix as a total function (V2-R02): exhaustive over `(Class, Capability)` via the private `Target → Class` mapping, so the compiler proves every capability has a cell for every target class — adding a capability appends a row, never edits one (D39)
- **`capability_matrix(target)`** — the `BTreeMap` fold of `support` over `Capability::ALL` (the machine-readable source of truth)
- **`degradation(target, capability)`** — why a `Degraded` cell degrades; every `D` in the matrix documents its reason
- **`emit(target, artifact, out_dir)`** — the deployable artifact: manifests, `_headers`/`_redirects` for static (§10.5), `Dockerfile` for docker, Worker entry + `_routes.json` + `wrangler.toml` for Cloudflare, handler wrappers + platform configs for serverless, `deploy.json` everywhere
- **`check_requirements` (the §18 refusal rule)** — a server manifest's `"requires": {"sockets": true}`-style declarations refuse the build on targets where the matching capability is `Unsupported`; `Degraded` capabilities proceed with the documented degradation string

## Usage

```rust
use oxide_fw_adapters::{Capability, Target, capability_matrix, support};

// One cell, two spellings — the map and the function cannot disagree:
assert_eq!(
    capability_matrix(Target::Static).get(&Capability::Live),
    Some(&support(Target::Static, Capability::Live)),
);
// Static has no server to hold a stream — the build refuses `live: true`.
assert_eq!(
    support(Target::Static, Capability::Live),
    oxide_fw_adapters::FeatureSupport::Unsupported,
);
```

## How it fits in Oxide

`oxide-fw-cli` calls `emit` at the end of `oxide build` for the configured adapter target, and the config layer's `live.transport` documentation points at `capability_matrix` for the serverless `poll` fallback (`oxide-fw-config`'s `LiveConfig`). The framework renderers themselves live in `oxide-fw-jshost` (the tooling pool); this crate owns the deploy surface, not the render path.

## See also

- [`oxide-fw-jshost`](https://crates.io/crates/oxide-fw-jshost) — the worker pool that runs the framework renderers
- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — the post-render HTML stream pipeline
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the HTTP pipeline that drives rendering

---

Full API reference: [docs.rs/oxide-fw-adapters](https://docs.rs/oxide-fw-adapters)
