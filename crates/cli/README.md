<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-cli

**The `oxide` command-line tool.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-cli.svg)](https://crates.io/crates/oxide-fw-cli) [![docs.rs](https://docs.rs/oxide-fw-cli/badge.svg)](https://docs.rs/oxide-fw-cli)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Everything the framework does at the operational level: dev server, production build, serve, checks, migrations, analysis, and health. The CLI is the orchestration layer — it builds the framework stack, wires the dispatchers, and runs the dev / build / serve loop.

## What it provides

The commands:

- **`oxide dev`** — dev server with HMR (CSS hot-swap, module reload, route-data updates)
- **`oxide build`** — production build (client + SSR + prerender + typegen)
- **`oxide serve`** — serve a built project
- **`oxide check`** — config validation, route scan, typecheck, lint, plugin diagnostics
- **`oxide routes`** — print the route table with render modes and L0 eligibility
- **`oxide analyze`** — per-route JS byte budget report
- **`oxide migrate`** — codemod from Next / Nuxt / SvelteKit / Vite
- **`oxide doctor`** — environment and config health
- **`oxide add <pkg>`** — install and register an Oxide module
- **`oxide generate`** — scaffold a route / layout / action
- **`oxide test`** — gate on build manifests
- **`oxide audit`** — OSV advisory scan against package-lock.json
- **`oxide schema`** — print the JSON Schema for `oxide.config.ts`
- **`oxide mcp`** — MCP server over stdio
- **`oxide render`** — render one route to HTML, bypass the server
- **`oxide config`** — print the loaded config as JSON
- **`oxide cache`** — inspect, clear, stats, gc
- **`oxide secrets`** — rotate the build secret

## Usage

```bash
oxide dev
oxide build
oxide serve --port 3000
oxide check --strict
```

Every command is also a public `cmd_*` function — tests and tooling can call them directly.

## How it fits in Oxide

The CLI is the only crate that sees the whole framework stack. It builds the renderer, the loader, the action dispatcher, the endpoint dispatcher, and the middleware dispatcher — then hands them to `oxide-fw-server` as trait objects. Everything else is library code that the CLI wires together.

## See also

- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the HTTP pipeline the CLI runs
- [`oxide-fw-dev`](https://crates.io/crates/oxide-fw-dev) — HMR and file watching
- [`oxide-fw-config`](https://crates.io/crates/oxide-fw-config) — config loading

---

Full API reference: [docs.rs/oxide-fw-cli](https://docs.rs/oxide-fw-cli)
