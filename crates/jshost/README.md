<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-jshost

**V8 and Node worker hosts for running user JavaScript.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-jshost?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-jshost) [![Downloads](https://img.shields.io/crates/d/oxide-fw-jshost?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-jshost) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-jshost?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-jshost) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-jshost?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-jshost?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-jshost)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Oxide's core is Rust; user code is JavaScript. This crate is the bridge. It runs `page.tsx`, `page.server.ts`, `route.ts`, `middleware.ts`, and `action.ts` in isolated V8 isolates or Node worker threads, with pooled-per-route affinity so a route's module cache stays warm.

The tooling pool (Vue, Svelte, and Solid compilers) is **always Node** — those packages need Node built-ins the V8 sandbox does not provide. The render/loader pool honors `server.backend` and defaults to V8, which starts in ~5 ms per isolate versus ~240 ms per Node child.

## What it provides

- **`Pool` trait** — the async `call(entry, input) -> CallStream` interface
- **`V8Host`** — real `deno_core` isolates, one per worker thread
- **`NodeHost`** — Node child-process pool
- **`FrameworkHostRenderer`** — the framework-adapter entry, loaded once per pool
- **`LoaderHost`, `ActionHost`, `EndpointHost`, `MiddlewareHost`, `SocketHost`** — typed wrappers over the worker's `*:run` ops
- **`ComponentSource`** — the generated SSR entry
- **Worker snapshot creation** — startup snapshot for fast cold start (spec §4.3)
- **`route_affinity`** — deterministic route → worker mapping so the same route lands on the same worker

## Usage

```rust,ignore
use oxide_fw_jshost::{JsHost, NodeHost, Pool, PoolConfig, CallInput, EntryRef};

let host = NodeHost::new(NodeHost::worker_script_default());
let pool = host.tooling_pool(PoolConfig::default())?;
let mut stream = pool.call(
    EntryRef { kind: "tooling:compile".into(), module: "/abs/vue-sfc".into(), export: "default".into() },
    CallInput { payload: serde_json::json!({}), headers: Default::default(), has_body: false, request_id: "req-1".into(), route: None },
).await?;
let bytes = stream.collect().await?;
```

> **Important:** The V8 sandbox has no `require('stream')` or other Node built-ins. Packages that depend on them (Vue's `@vue/compiler-sfc`, `svelte/compiler`, Tailwind's `@tailwindcss/oxide`) must run on the Node pool, not the V8 pool. Oxide's `server.backend` config controls the render pool; the tooling pool is always Node.

## How it fits in Oxide

`oxide-fw-cli` builds two pools during `cmd_dev` / `cmd_build` / `cmd_serve`:
- **Render/loader pool** — where the framework adapter runs. Backend chosen by `server.backend` (defaults to V8).
- **Tooling pool** — always Node. Used by the bundler (SFC precompile), Tailwind v4 bridge, and the six JS plugin hook builders.

`oxide-fw-adapters` consumes the render pool through `FrameworkHostRenderer`, wrapped in `AdapterPageRenderer` so the pipeline sees one `PageRenderer` trait object.

## See also

- [`oxide-fw-adapters`](https://crates.io/crates/oxide-fw-adapters) — the framework bridge that uses the pool
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — consumes `LoaderRunner` / `ActionDispatcher` / etc. from this crate
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — constructs the pools per project config

---

Full API reference: [docs.rs/oxide-fw-jshost](https://docs.rs/oxide-fw-jshost)
