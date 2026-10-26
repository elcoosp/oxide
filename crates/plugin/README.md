# oxide-fw-plugin

**WASM and JS plugin system with a typed hook ABI.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-plugin.svg)](https://crates.io/crates/oxide-fw-plugin) [![docs.rs](https://docs.rs/oxide-fw-plugin/badge.svg)](https://docs.rs/oxide-fw-plugin)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Third-party extension points. A plugin can be a WASM component (with declared capabilities) or a JavaScript module. Both expose the same hook ABI so the bundler and prerender pipeline do not need to know which kind of plugin they are calling.

The plugin system is deliberately small. There are 19 hooks, each with a well-defined chaining rule. `transform` chains code (output A → input B); `resolveId` first-non-null wins; `load` first-non-null wins; `generateBundle` last-non-null wins; lifecycle hooks just need to run. The ABI is `string -> string` for the interesting hooks — no shared state, no constructor, no "reset".

## What it provides

- **`Hook`** — the enum (19 variants: Config, ConfigResolved, BuildStart, ResolveId, Load, Transform, RenderChunk, GenerateBundle, BuildEnd, TransformHtml, HandleHotUpdate, ConfigureServer, Routes, Adapter, ServerMiddleware, RenderHead, CacheHandler, Typegen, Diagnostics)
- **`WasmPlugin`** — load a `.wasm` component against a `WasmPluginManifest`
- **`call_string_hook`** — call a hook and get the transformed source back
- **`available_hooks`** — which of the 19 the loaded component declares
- **Capability model** — a plugin declares what it needs; the host enforces it

## Usage

```rust
use oxide_fw_plugin::{WasmPlugin, WasmPluginManifest, Hook};

let plugin = WasmPlugin::load(
    &std::fs::read("plugin.wasm")?,
    WasmPluginManifest {
        name: "my-plugin".into(),
        wit_world: "oxide:plugin/hooks".into(),
        capabilities: vec![],
        hooks: vec![Hook::Transform],
    },
    None,
)?;
let out = plugin.call_string_hook(&Hook::Transform, source)?;
```

> [!TIP]
> Feature-gate the `wasm` support (which pulls `wasmtime`) so a project that only ships JS plugins builds without it. The CLI does this via a per-plugin `kind` field in `oxide.config.ts`.

## How it fits in Oxide

`oxide-fw-cli` loads plugins in three places:
- **Bundler hooks** (`transform`, `resolveId`, `load`, `renderChunk`, `generateBundle`, `transformHtml`) — via the six `build_js_plugin_*` / `build_transform_hook` builders.
- **Lifecycle hooks** (`buildStart`, `buildEnd`, `configureServer`, `handleHotUpdate`) — via `fire_lifecycle_hook`.
- **Diagnostic + metadata hooks** (`diagnostics`, `renderHead`, `cacheHandler`, `typegen`) — via `plugin_declares_any` gates.

Every call goes through the tooling pool (Node or V8) so the JS plugin ABI matches the WASM plugin ABI from the caller's perspective.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — loads and invokes plugins
- [`oxide-fw-core-wasm`](https://crates.io/crates/oxide-fw-core-wasm) — the WASM component loading contract
- [`oxide-fw-bundler`](https://crates.io/crates/oxide-fw-bundler) — the six hook surfaces the bundler exposes

---

Full API reference: [docs.rs/oxide-fw-plugin](https://docs.rs/oxide-fw-plugin)
