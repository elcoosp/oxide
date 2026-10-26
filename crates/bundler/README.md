# oxide-fw-bundler

**Production bundler built on Rolldown and Oxc.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-bundler.svg)](https://crates.io/crates/oxide-fw-bundler) [![docs.rs](https://docs.rs/oxide-fw-bundler/badge.svg)](https://docs.rs/oxide-fw-bundler)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The JavaScript and TypeScript bundler Oxide uses for both client and server output. Rolldown is a Rust-native bundler; Oxide integrates via a plugin bridge so the framework's own passes (such as `'use cache'`, `serverFn` stubs, and server-only guards) compose with Rolldown's normal lowering.

## What it provides

- **`RolldownBundler`** — the `Bundler` trait implementation
- **Six plugin hook surfaces** — `transform`, `resolveId`, `load`, `renderChunk`, `generateBundle`, `transformHtml`
- **Virtual modules** — for `?react` and `?w=` SVG/image imports
- **Per-chunk code splitting** — content-hashed filenames
- **Sourcemaps** — `off` | `file` | `hidden`
- **Rollup-style plugin compatibility** — via Rolldown's Rust ABI

## Usage

```rust
use oxide_fw_bundler::{BundleEnv, BundleOptions, Bundler, RolldownBundler};

let opts = BundleOptions {
    env: BundleEnv::Client,
    entries: /* route id -> source path */,
    // ...
};
let out = RolldownBundler::new(root).bundle(&opts)?;
for chunk in out.chunks {
    println!("{}  {}", chunk.file, chunk.kind_str());
}
```

> [!NOTE]
> This crate does not depend on `oxide-fw-plugin` — that would pull `wasmtime` into every build. The CLI pre-loads plugins and hands in closures over `BundleOptions` instead.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_build` constructs one `BundleOptions` per env (client, SSR, edge) and calls `bundle()`. The bundler knows nothing about frameworks, routing, or Tailwind — it knows JS, CSS, and the six hook surfaces.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — builds `BundleOptions` and drives the bundler
- [`oxide-fw-compiler`](https://crates.io/crates/oxide-fw-compiler) — the framework-specific source passes
- [`oxide-fw-assets`](https://crates.io/crates/oxide-fw-assets) — the CSS transform hook

---

Full API reference: [docs.rs/oxide-fw-bundler](https://docs.rs/oxide-fw-bundler)
