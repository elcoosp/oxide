<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-schema

**JSON Schema generation for oxide.config.ts — editor autocomplete and CI validation.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-schema?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-schema) [![Downloads](https://img.shields.io/crates/d/oxide-fw-schema?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-schema) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-schema?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-schema) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-schema?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-schema?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-schema)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The schema this crate emits is the contract between a user's `oxide.config.ts` and their editor. Add `"$schema": "https://oxide.dev/config.schema.json"` to a config (or a `json.schemas` setting for `.ts` files) and the editor validates every field, suggests completions, and shows inline documentation.

The schema is also what `oxide schema` prints on stdout, so a CI step can diff it against a committed file and fail when the config surface changes.

## What it provides

- **`schema_pretty()`** — the full JSON Schema as a pretty-printed string
- **`schema_value()`** — the same schema as a `serde_json::Value`
- **`schema_url()`** — the canonical URL a config file should reference
- **`SCHEMA_URL`** — the const for tools that embed the URL

## Usage

```rust,ignore
use oxide_fw_schema::{schema_pretty, schema_url};

println!("{}", schema_pretty()?);
// → { "$schema": "https://json-schema.org/draft/2020-12/schema", ... }

println!("{}", schema_url());
// → https://oxide.dev/config.schema.json
```

## How it fits in Oxide

`oxide-fw-config` owns the config surface and generates the schema. This crate wraps that generation in a stable, versioned public API. `oxide-fw-cli`'s `oxide schema` command calls `schema_pretty()` and writes the result to stdout.

## See also

- [`oxide-fw-config`](https://crates.io/crates/oxide-fw-config) — the config surface the schema describes
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — the `oxide schema` command
- [`oxide-fw-diagnostics`](https://crates.io/crates/oxide-fw-diagnostics) — the error type

---

Full API reference: [docs.rs/oxide-fw-schema](https://docs.rs/oxide-fw-schema)
