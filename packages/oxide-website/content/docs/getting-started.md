---
title: Getting started
description: Build the single binary, scaffold an app, and run the dev loop.
order: 1
---

# Getting started

Oxide is a full-stack meta-framework with a **Rust core**: React, Vue, Svelte and Solid are
first-class adapters over one shared engine that owns routing, data, caching, bundling, serving
and deployment. This page takes you from zero to a running app.

## Prerequisites

- A **Rust stable** toolchain (edition 2024) — [rustup](https://rustup.rs) if you don't have it
- **Node ≥ 24** — the TypeScript packages have zero runtime npm dependencies and run on Node's
  native type stripping, so there is no transpile step

## 1. Build the single binary

```sh
git clone https://github.com/elcoosp/oxide
cd oxide
cargo build --release -p oxide-cli
target/release/oxide-cli doctor   # 7 environment checks: node, cargo, config, routes dir, cache dir, secrets, cookie keys
```

One binary. Everything else in this guide is a subcommand of it.

## 2. Scaffold an app

Sixteen templates — 4 frameworks × minimal / blog / dashboard / storefront, each shipping a
streaming `defer()` demo:

```sh
node packages/create-oxide/src/cli.ts blog react my-app
```

## 3. Run the dev loop

```sh
cd my-app
oxide dev       # dev server + watcher + HMR hub + inspector; --https --qr --open --proxy
oxide routes    # scan routes, infer render modes, emit typegen, print the route table
oxide build     # bundle, prerender, emit deploy artifacts
oxide start     # serve; health at /_oxide/health
```

While `dev` runs, the inspector shows request waterfalls (also on `Server-Timing`, and via the
`oxide mcp` server for AI tooling).

## Project structure

- `oxide.config.ts` — typed config, statically extracted; `oxide schema` emits a JSON Schema you
  can reference via `$schema` for editor completions
- `src/routes/` — file conventions: `page.tsx`, `page.server.ts` (loaders), `layout.tsx`,
  `loading.tsx`, `error.tsx`, `not-found.tsx`, `route.ts` endpoints
- `public/` — static files, served root-absolute

## Next steps

- [Architecture](/docs/architecture) — what runs where, and why the hot path skips JavaScript
- [Wire format (ODF)](/docs/wire-format-odf) — the byte-parity format under the cache and the wire
- [Caching](/docs/caching) — L0/L1/L2, tags, stale-while-revalidate, single-flight
- Star or watch [the repo](https://github.com/elcoosp/oxide) — or read the
  [public compliance scorecard](https://github.com/elcoosp/oxide) to see exactly where v0.1 stands
