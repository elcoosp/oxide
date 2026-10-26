<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-config

**TypeScript config loading, extraction, and schema validation.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-config?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-config) [![Downloads](https://img.shields.io/crates/d/oxide-fw-config?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-config) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-config?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-config) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-config?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-config?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-config)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Oxide's configuration is `oxide.config.ts` — a real TypeScript file, not a JSON subset. This crate evaluates it, extracts what it can statically, validates the result against a JSON Schema, and hands a strongly-typed `OxideConfig` to the rest of the framework.

The file is loaded by a Node worker (the "framework host") that imports the module, runs the user's `createAdapter(...)` factory if present, and returns a serialized descriptor. A static extractor fast-paths the literal-object case.

## What it provides

- **`load(root, env, env_vars)`** — full config load through the framework host
- **Static extraction** — fast path for literal-object configs
- **`OxideConfig`** — the typed view: `framework`, `routes`, `server`, `cache`, `security`, `i18n`, `plugins`, `modules`, `budgets`, `adapter`, …
- **`config_schema_pretty()`** — JSON Schema for editor autocomplete
- **`parse_size`** — human-readable sizes (`"512mb"`, `"64kb"`) with overflow checks
- **Layered resolution** — `oxide.config.{ts,mjs,js}` with per-env overrides

## Usage

```rust
use oxide_fw_config::load;
use std::collections::BTreeMap;

let loaded = load(&root, "production", BTreeMap::new())?;
println!("framework adapter: {:?}", loaded.config.framework.adapter);
```

> **Tip:** Add `"$schema": "https://oxide.dev/config.schema.json"` to your `oxide.config.json` (or an editor setting for `.ts`) to get full autocomplete and inline docs in VS Code.

## How it fits in Oxide

Every `oxide` command loads the config first. `oxide-fw-cli` calls `load` (or `load_config_or_default` for commands that tolerate a missing config). The framework stack, route scanner, bundler, and server all read from the typed `OxideConfig` — none of them re-parse `oxide.config.ts` themselves.

## See also

- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — the primary consumer
- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — reads `security`, `cache`, `server` fields
- [`oxide-fw-assets`](https://crates.io/crates/oxide-fw-assets) — reads `i18n`, `images`, `fonts`

---

Full API reference: [docs.rs/oxide-fw-config](https://docs.rs/oxide-fw-config)
