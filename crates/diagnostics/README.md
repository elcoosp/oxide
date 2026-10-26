<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-diagnostics

**The framework's error type — `OxideError` and `Diagnostic`.**

[![Crates.io](https://crates.io/crates/oxide-fw-diagnostics) `soon`] [![docs.rs](https://docs.rs/oxide-fw-diagnostics/badge.svg)](https://docs.rs/oxide-fw-diagnostics)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Every failed operation in Oxide returns an `OxideError`, which wraps a boxed `Diagnostic`. The diagnostic carries a stable error code (`CFG-FRAMEWORK`, `BLD-CSS`, `JSH-OVERFLOW`, …), a severity, an optional location, an optional code frame, and an optional fix. Because the diagnostic is boxed, the `Err` variant is 8 bytes — a `Result<T, OxideError>` stays cheap even when `T` is a small Copy type.

The code families are stable across releases, so a downstream tool can classify a failure by prefix alone (e.g. `CFG-*` for config, `BLD-*` for build, `JSH-*` for the JS host).

## What it provides

- **`Diagnostic`** — the structured error shape
- **`OxideError`** — a thin newtype used across every crate's `Result` alias
- **`Severity`** — Error / Warning / Info
- **`Location`** — file, line, column with span information
- **Code-frame rendering** — for terminal and editor display
- **`Diagnostic::error(code, message)`** — the standard constructor
- **`.warning(...)`, `.at(loc)`, `.with_fix(...)`** — builder methods
- **`impl From<Diagnostic> for OxideError`** and **`From<Box<Diagnostic>>`** — so a `?` conversion works in either direction

## Usage

```rust
use oxide_fw_diagnostics::{Diagnostic, Location, OxideError};

let err = OxideError::new("CFG-FRAMEWORK", "unknown adapter `foo`")
    .at(Location::file("oxide.config.ts", 3, 5))
    .with_fix("did you mean `vue`?");
```

> **Note:** The `Box<Diagnostic>` inside `OxideError` is the reason the workspace-wide `clippy::result_large_err` count is zero. A `Result<(), OxideError>` return is 8 bytes, not 150+ — so every `Ok` return on every hot path pays a fixed 8-byte cost, not a 150-byte one.

## How it fits in Oxide

Every crate in the workspace depends on `oxide-fw-diagnostics`. The `Result<T>` alias is the standard signature for every fallible API. `oxide-fw-cli` collects diagnostics into `.oxide/last-build-diagnostics.json` so the editor and CI read one artifact regardless of which stage failed.

## See also

- Every other crate in the workspace — this is the shared error type
- [`miette`](https://crates.io/crates/miette) — pretty-printing integration via the `fancy` feature

---

Full API reference: [docs.rs/oxide-fw-diagnostics](https://docs.rs/oxide-fw-diagnostics)
