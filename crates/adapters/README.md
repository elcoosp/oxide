# oxide-fw-adapters

**Framework adapters for React, Vue, Svelte, and Solid.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-adapters.svg)](https://crates.io/crates/oxide-fw-adapters) [![docs.rs](https://docs.rs/oxide-fw-adapters/badge.svg)](https://docs.rs/oxide-fw-adapters)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The bridging layer between the Oxide Rust render pipeline and each framework's server-side renderer. Consumes the pipeline's neutral `PageRenderInput` shape and produces an HTML stream from the framework's own SSR machinery.

## What it provides

- **`FrameworkRenderer` trait** — one entry point per framework
- **`AdapterPageRenderer`** — accepts the pipeline's neutral input shape, wraps any `FrameworkRenderer`
- **`LazyFrameworkRenderer`** — defers config evaluation to first render (~325 ms saved at boot)
- **JSX runtime resolution** — Solid's `@solidjs/web`, React's `react/jsx-runtime`
- **SFC precompile contracts** — `.vue` and `.svelte` sources
- **Per-framework feature gates** — enable only the adapter you use

## Usage

```rust
use oxide_fw_adapters::{AdapterPageRenderer, FrameworkRenderer};

let inner: Arc<dyn FrameworkRenderer> = /* from oxide-fw-jshost */;
let renderer = AdapterPageRenderer::new(inner);
let stream = renderer.render_streaming(input).await?;
```

## How it fits in Oxide

`oxide-fw-jshost` hosts the actual framework renderer inside a Node or V8 pool. This crate adapts between the pipeline's `PageRenderInput` (neutral) and the framework's own input shape. `oxide-fw-cli` wires the two together in `build_framework_stack`.

## See also

- [`oxide-fw-jshost`](https://crates.io/crates/oxide-fw-jshost) — the worker pool that runs the renderer
- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — the post-render HTML stream pipeline
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the HTTP pipeline that drives rendering

---

Full API reference: [docs.rs/oxide-fw-adapters](https://docs.rs/oxide-fw-adapters)
