---
title: CLI reference
description: Every subcommand, generated from the Command enum in crates/cli/src/lib.rs with line numbers.
order: 6
---

# CLI reference

One binary. Every subcommand is a variant of `Command` in
`crates/cli/src/lib.rs:73-234`. A flag not in the enum does not exist.

## Core loop

| Command | Enum line | What it does |
|---|---|---|
| `oxide dev` | `crates/cli/src/lib.rs:73` | server + watcher + HMR hub + inspector |
| `oxide build` | `crates/cli/src/lib.rs:102` | bundle, prerender, typegen, emit artifacts |
| `oxide start` | `crates/cli/src/lib.rs:115` | production server; `--root`, `--port` |
| `oxide preview` | `crates/cli/src/lib.rs:122` | alias of start, no watcher |
| `oxide render` | `crates/cli/src/lib.rs:234` | dispatch a page render, HTML to stdout |

`oxide build` runs the full pipeline: client bundle, SSR bundle, prerender,
typegen, rkyv route manifest, V8 snapshot, L0 cache seed, forbidden-import
guard, and deploy artifact emission through `crates/adapters/src/lib.rs:222`
(`emit`).

## Introspection

| Command | Enum line | What it does |
|---|---|---|
| `oxide routes` | `crates/cli/src/lib.rs:127` | scan routes, infer modes, emit typegen, print table |
| `oxide analyze` | `crates/cli/src/lib.rs:132` | per-route JS bytes vs `config.budgets` |
| `oxide check` | `crates/cli/src/lib.rs:137` | config + route diagnostics, typecheck, lint |
| `oxide why` | `crates/cli/src/lib.rs:142` | which routes import a module, which chunks contain it |
| `oxide doctor` | `crates/cli/src/lib.rs:148` | seven environment checks |

`oxide routes` calls `RouteTree::scan` (`crates/router/src/lib.rs:139`) and
prints the resulting table. `oxide doctor` checks the same seven things as the
`Doctor` variant: node, cargo, config, routes directory, cache directory,
build secret, cookie keys.

## Authoring

| Command | Enum line | What it does |
|---|---|---|
| `oxide generate` | `crates/cli/src/lib.rs:169` | scaffold route / layout / action files |
| `oxide add` | `crates/cli/src/lib.rs:162` | register a module in `.oxide/modules.json` |
| `oxide schema` | `crates/cli/src/lib.rs:217` | JSON Schema for `oxide.config.{ts,json}` |
| `oxide config` | `crates/cli/src/lib.rs:222` | load resolved config, print adapter descriptor |
| `oxide migrate` | `crates/cli/src/lib.rs:196` | codemods for Next, Nuxt, SvelteKit, Vite |

## Operations

| Command | Enum line | What it does |
|---|---|---|
| `oxide secrets` | `crates/cli/src/lib.rs:155` | rotate build secret; next build mints a fresh one |
| `oxide cache` | `crates/cli/src/lib.rs:177` | inspect / clear / stats on `.oxide/cache` |
| `oxide audit` | `crates/cli/src/lib.rs:185` | advisory pass over registered modules and plugins |
| `oxide test` | `crates/cli/src/lib.rs:205` | build-manifest gate for the app test helpers |
| `oxide mcp` | `crates/cli/src/lib.rs:211` | stdio MCP server |

## Where the commands write

Every subcommand that reads or writes project state goes through a handler in
`crates/cli/src/lib.rs` and a `Writer<'_>` so tests can capture output without
touching stdout. The pipeline types it constructs:

| Type | File:line | Used by |
|---|---|---|
| `OxideRequest` | `crates/server/src/lib.rs:59` | `oxide render` |
| `OxideResponse` | `crates/server/src/lib.rs:187` | `oxide render` |
| `Limits` | `crates/server/src/lib.rs:263` | `oxide check` |
| `StaticAssets` | `crates/server/src/lib.rs:286` | `oxide analyze` |
| `CacheHandler` | `crates/cache/src/lib.rs:196` | `oxide cache` |

## Exit codes

- `0` — success
- `1` — a check or build failed; the diagnostic on stderr carries a code such
  as `ADP-STATIC-DYN`, emitted by `degradation` at
  `crates/adapters/src/lib.rs:154`
- `2` — usage error, or an audit that found an advisory
