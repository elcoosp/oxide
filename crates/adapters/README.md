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
- **`capability_markdown_table()`** — the §18 matrix rendered as a deterministic markdown table (E11's generated docs table): the one embedded below, byte-identical, pinned by `tests/doc_table_tdd.rs::doc_table_matches_the_function`
- **`degradation(target, capability)`** — why a `Degraded` cell degrades; every `D` in the matrix documents its reason
- **`Capability::as_str()`** — the kebab-case id the doctor column, `build --explain` and the generated docs table print
- **`emit(target, artifact, out_dir)`** — the deployable artifact: manifests, `_headers`/`_redirects` for static (§10.5), `Dockerfile` for docker, Worker entry + `_routes.json` + `wrangler.toml` for Cloudflare, handler wrappers + platform configs for serverless, `deploy.json` everywhere
- **`check_requirements` (the §18 refusal rule, manifest half)** — a server manifest's `"requires": {"sockets": true}`-style declarations refuse the build on targets where the matching capability is `Unsupported`; `Degraded` capabilities proceed with the documented degradation string
- **`check_config` + `ConfigFeatureReport` (the §18 refusal rule, config half — V2-R02's config-key remainder, spec §V2-R02)** — the config layer's feature report (`ConfigFeatureReport`: live routes, `*.server.rs` presence, the `search` key (F09), `fonts.subset: 'used'`, non-static routes) turned into registry-code refusals (`AST-LIV-001`, `RFN-002`, `AST-SRC-001`, `AST-FNT-001`) and degradation warnings against the same matrix; the config crate maps its keys, the CLI build refuses before any deploy-artifact work
- **`deploy_dir` / `plan` / `prune` / `rollback` + `DeployStore` / `DirStore` (the delta-deploy engine — spec §V2-F11)** — upload only what changed and swap atomically: the pure `plan` diff over blake3-hashed scans, `DirStore` as the `dir:` target over `std::fs` (temp+rename pointer swap, traversal-guarded keys), `deploy_dir`'s upload-then-pointer-last order of operations (immutable chunks at rel paths, non-hashed files mirrored under `releases/<build-id>/`), `prune` with the F10 default 15-minute skew window and the rollback-safety rule, `rollback`'s one-write pointer swap
- **`ObjectStoreStore` + `deploy_target_from_url` (the object-store deploy target — the §V2-F11 remaining half, behind the `s3` feature)** — the second `DeployStore` impl: `s3://bucket/prefix`, `r2://bucket/prefix?endpoint=…` (the S3 API against an R2 endpoint override), `file://` and `mem://` backends through `object_store`; the pointer swap is a conditional put (create-if-absent on a fresh store, update-against-the-read-version after), so a swap that loses the race refuses with `DEP-POINTER` and never flips `current`

## Capability matrix

Generated by `capability_markdown_table()` — the docs and the function cannot disagree (E11's `doc_table_matches_matrix` pin). Cells: `F` full, `D` degraded (the documented reason is `degradation(target, capability)`), `U` unsupported (the §18 refusal rule covers it at build time — `check_requirements` for server manifests, `check_config` for config keys).

| target | l0-cache | isr | sockets | image-optimization | native-rules | snapshots | tls | live | mixed-islands | resume | skew | delta-deploy | rust-fns | search | offline | brownout | permissions | attestation |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| binary | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F |
| node | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |
| static | D | U | U | D | D | U | U | U | F | U | D | F | U | D | F | U | U | F |
| docker | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F | F |
| cloudflare | D | D | U | U | F | U | F | D | D | U | D | D | U | U | F | U | U | F |
| cloudflare-pages | D | D | U | U | F | U | F | D | D | U | D | D | U | U | F | U | U | F |
| lambda | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| vercel | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| vercel-functions | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| netlify | D | D | U | D | F | D | U | D | F | U | D | D | U | D | F | U | U | F |
| deno | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |
| bun | F | F | F | F | F | D | F | F | F | F | D | D | U | D | F | D | D | F |

## Delta deploys (V2-F11)

`deploy_dir` uploads only what changed and swaps atomically. The engine is store-agnostic: `DeployStore` is the seam — the upload ledger (`RemoteManifest`), object put/delete, the atomic `current` pointer — `DirStore` implements the `dir:` target over `std::fs`, and `ObjectStoreStore` (behind the `s3` feature) implements the `s3://` / `r2://` target over `object_store` (`deploy_target_from_url` parses the target spelling; `file://` and `mem://` ride the same parser for local and test deploys). On the object-store target the pointer swap is a conditional put: create-if-absent on a fresh store, update against the version the pointer carried when this deploy read it — a swap that loses the race refuses with `DEP-POINTER` instead of overwriting a concurrent deploy's pointer. Immutable hashed chunks (`assets/`, `_oxide/`) are stored at their rel path, so identical bytes across builds upload once; every non-hashed file mirrors into `releases/<build-id>/`, so a release is self-contained for the tabs reading the new pointer; the pointer swap is the LAST write, so a deploy that fails mid-upload never flips production. `prune` removes superseded files after the skew window (15 minutes by default) and never touches the release the pointer names — the rollback-safety rule. `rollback` re-points `current` to the previous build id in one small write.

```text
cargo run -p oxide-fw-adapters --example delta_deploy
cargo run -p oxide-fw-adapters --features s3 --example object_store_deploy
```

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
