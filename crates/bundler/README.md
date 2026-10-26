<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-bundler

**Production bundler built on Rolldown and Oxc.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-bundler?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-bundler) [![Downloads](https://img.shields.io/crates/d/oxide-fw-bundler?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-bundler) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-bundler?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-bundler) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-bundler?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-bundler?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-bundler)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

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

> **Note:** This crate does not depend on `oxide-fw-plugin` — that would pull `wasmtime` into every build. The CLI pre-loads plugins and hands in closures over `BundleOptions` instead.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_build` constructs one `BundleOptions` per env (client, SSR, edge) and calls `bundle()`. The bundler knows nothing about frameworks, routing, or Tailwind — it knows JS, CSS, and the six hook surfaces.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — builds `BundleOptions` and drives the bundler
- [`oxide-fw-compiler`](https://crates.io/crates/oxide-fw-compiler) — the framework-specific source passes
- [`oxide-fw-assets`](https://crates.io/crates/oxide-fw-assets) — the CSS transform hook

---

Full API reference: [docs.rs/oxide-fw-bundler](https://docs.rs/oxide-fw-bundler)
