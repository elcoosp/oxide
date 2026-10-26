<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-core-wasm

**Minimal WASM core support.**

[![Crates.io](https://crates.io/crates/oxide-fw-core-wasm) `soon`] [![docs.rs](https://docs.rs/oxide-fw-core-wasm/badge.svg)](https://docs.rs/oxide-fw-core-wasm)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

A small crate — four public items — that provides the WASM component loading contract used by the plugin and adapter layers. Split out to keep the main pipeline crates free of a `wasmtime` dependency for builds that never load a WASM component.

## What it provides

- **Component loading contract** — the shape a WASM plugin's `.wasm` file is loaded and instantiated against
- **Capability declarations** — the metadata a plugin provides about what host services it needs
- **Failure typing** — a stable error code family for a component that fails to load

## How it fits in Oxide

Most users will not depend on this crate directly. It is pulled in transitively by `oxide-fw-plugin` when the `wasm` feature is enabled. If your project never loads a WASM plugin, `oxide-fw-core-wasm` is not built into your binary.

## See also

- [`oxide-fw-plugin`](https://crates.io/crates/oxide-fw-plugin) — the plugin system that consumes this crate
- [`wasmtime`](https://crates.io/crates/wasmtime) — the runtime this crate builds on

---

Full API reference: [docs.rs/oxide-fw-core-wasm](https://docs.rs/oxide-fw-core-wasm)
