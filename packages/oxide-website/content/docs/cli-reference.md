---
title: CLI reference
description: One binary, twenty subcommands — dev, build, start, introspection, authoring and operations.
order: 6
---

# CLI reference

Everything in Oxide is a subcommand of one binary. `oxide doctor` sanity-checks the environment.

## Core loop

| Command | What it does |
| --- | --- |
| `dev` | Dev server + watcher + HMR hub + inspector; per-file invalidation, idle pre-warm; `--https --qr --open --proxy` |
| `build` | Client+SSR bundles, prerender, typegen, `rkyv` route manifest, V8 snapshot, L0 cache seed, deploy artifacts, forbidden-import guard |
| `start` · `preview` | Production server; health at `/_oxide/health` |
| `render <route>` | Dispatch a page render, HTML to stdout (`--url --param --data --stream`) |

## Introspection

| Command | What it does |
| --- | --- |
| `routes` | Scan routes, infer render modes, emit typegen, print the route table |
| `analyze` | Per-route JS bytes vs `config.budgets` — exits non-zero on breach; top modules by reachable bytes |
| `check` | Config + route diagnostics; typechecks (tsgo) and lints (oxlint) when on PATH |
| `why <module>` | Which routes import a module, which chunks contain it |
| `doctor` | 7 environment checks: node, cargo, config, routes dir, cache dir, secrets, cookie keys |

## Authoring

| Command | What it does |
| --- | --- |
| `generate` | Scaffold route / layout / action files per framework |
| `add <module>` | Register a module in `.oxide/modules.json` |
| `schema` | JSON Schema for `oxide.config.{ts,json}` |
| `config` | Load the resolved config, print the framework adapter descriptor |
| `migrate <source>` | Migration codemods for Next, Nuxt, SvelteKit, Vite (`--write` to apply) |

## Operations

| Command | What it does |
| --- | --- |
| `secrets rotate` | Drop the build secret; the next build mints a fresh one |
| `cache inspect\|clear\|stats` | Inspect, clear or measure `.oxide/cache` |
| `audit` | Static advisory pass over registered modules and plugins (live OSV queries with a 24h cache) |
| `test` | Build-manifest gate for the app test helpers |
| `mcp` | stdio MCP server exposing `oxide_routes`, `oxide_cache`, `oxide_graph`, `oxide_errors` |

## Scaffolding

`create-oxide` emits one of 16 templates (4 frameworks × minimal / blog / dashboard / storefront):

```sh
node packages/create-oxide/src/cli.ts <template> <framework> <dir>
node packages/create-oxide/src/cli.ts blog svelte my-app
```
