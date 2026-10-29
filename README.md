<div align="center">
  <img src="docs/logo.png" alt="Oxide" width="120"/>
  <h1>Oxide</h1>
  <p><strong>One Rust core. Every framework.</strong></p>
  <p>
    <a href="https://github.com/elcoosp/oxide/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/CI-Linux%20%7C%20macOS%20%7C%20Windows-9D4EDD?style=flat-square&logo=github" alt="CI"/></a>
    <a href="https://github.com/elcoosp/oxide/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-MIT%20%7C%20Apache--2.0-blue?style=flat-square" alt="License"/></a>
    <a href="https://crates.io/crates/oxide-fw-cli"><img src="https://img.shields.io/badge/Rust-stable%20%7C%20edition%202024-000000?style=flat-square&logo=rust" alt="Rust"/></a>
    <a href="#framework-adapters"><img src="https://img.shields.io/badge/Adapters-React%20%7C%20Vue%20%7C%20Svelte%20%7C%20Solid-007ACC?style=flat-square" alt="Adapters"/></a>
    <a href="https://github.com/elcoosp/oxide/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/Tests-1239%20Rust%20%7C%20445%2F445%20TS-brightgreen?style=flat-square" alt="Tests"/></a>
  </p>
  <p>
    <a href="#install">Install</a> ·
    <a href="#features">Features</a> ·
    <a href="#architecture">Architecture</a> ·
    <a href="#framework-adapters">Adapters</a> ·
    <a href="#the-cli">CLI</a> ·
    <a href="#documentation">Docs</a> ·
    <a href="#project-status">Status</a>
  </p>
</div>

> [!NOTE]
> A demo GIF lands here on tag day. Until then:
> `cargo build --release -p oxide-fw-cli && ./target/release/oxide doctor`.

Oxide is a **full-stack meta-framework with a Rust core**. React, Vue, Svelte and Solid are first-class adapters over one shared engine that owns routing, data, caching, bundling, serving and deployment — so anything framework-neutral behaves identically on all four. It compiles to a single static binary and ships to twelve deploy targets.

## Install

Prerequisites: **Rust stable** (edition 2024) and **Node ≥ 24**.

```bash
cargo install oxide-fw-cli       # the CLI — one static binary
oxide doctor                     # env check: node, cargo, config, cache, secrets

pnpm create oxide my-app --framework react --template blog
cd my-app
oxide dev                        # dev server with HMR
oxide build                      # production build
oxide start --port 3000          # production server
```

> [!NOTE]
> **v0.1.0 is not yet published.** Until `cargo install oxide-fw-cli` resolves on crates.io, build from source — see [Building from source](#building-from-source). Every command above works the same against `./target/release/oxide`.

`create-oxide` is the separate npm scaffolder ([npmjs.com/package/create-oxide](https://www.npmjs.com/package/create-oxide)).

## Features

| | |
| --- | --- |
| **One engine, four frameworks** | One versioned adapter contract (`contract: 1`). Real streaming SSR is the default; framework libraries are optional peers. |
| **Fast paths skip JS** | Redirects, rewrites, headers, auth and rate limits evaluate natively in Rust. `oxide.config.ts` is statically extracted — the config path never runs JavaScript. |
| **One wire format** | ODF is a devalue-compatible binary format encoded in Rust, decoded by a zero-dependency TS module. `Date`, `Map`, `Set`, `BigInt` survive a cache round trip. |
| **Three-tier cache** | L0 in-process, L1 data cache, L2 pluggable remote (Redis, S3, HTTP). Tags, stale-while-revalidate, single-flight, MAC-sealed remote payloads. |
| **One static binary** | hyper 1.x + rustls: HTTP/1.1, HTTP/2, HTTP/3 over QUIC. Precompressed static serving (zstd → brotli → gzip), range requests, zero-downtime reload via SO_REUSEPORT. |
| **Twelve deploy targets** | Binary, Node, Static, Docker, Cloudflare (Workers + Pages), Lambda, Vercel (+ Functions), Netlify, Deno, Bun — behind a capability matrix that refuses at build time what the target cannot honour. |

## Architecture

```
request → hyper → limits → static → native rules → route match
        → L0 cache → middleware → loaders → dispatch → streaming SSR
```

Cached, redirected, rewritten and static answers **never enter a JavaScript runtime**. Four traits sit between the pipeline and every external engine:

| Trait | Purpose |
| --- | --- |
| `PageRenderer` | page module + data → HTML, buffered or streamed |
| `LoaderRunner` | run `page.server.ts` loaders in the layout chain |
| `MiddlewareDispatcher` | outer→inner; short-circuit, rewrite, or contribute `locals` |
| `ActionDispatcher` | form POST routed to a named action |

Eighteen Rust crates: `server` (hyper, static, compression, TLS), `render` (streaming SSR, HTML injection), `router` (tree, matcher, typegen), `cache` (three tiers), `odf` (wire format), `compiler` (Oxc passes), `bundler` (rolldown), `jshost` (V8 + Node), `graph` (incremental), `plugin` (Wasm), `adapters` (deploy), `cli`, plus `assets`, `config`, `dev`, `diagnostics`, `schema`, `core-wasm`.

## Framework adapters

| Adapter | Package | Extensions | Peer deps |
| --- | --- | --- | --- |
| React | [`@oxide-fw/react`](https://www.npmjs.com/package/@oxide-fw/react) | `.tsx` | `react@^19`, `react-dom@^19` |
| Vue | [`@oxide-fw/vue`](https://www.npmjs.com/package/@oxide-fw/vue) | `.vue` | `vue@^3.6`, `@vue/server-renderer@^3.6` |
| Svelte | [`@oxide-fw/svelte`](https://www.npmjs.com/package/@oxide-fw/svelte) | `.svelte` | `svelte@^5` |
| Solid | [`@oxide-fw/solid`](https://www.npmjs.com/package/@oxide-fw/solid) | `.tsx` | `solid-js@^2`, `@solidjs/web@^2` |

The loader is framework-neutral. It lives in `page.server.ts` and returns a plain object; only the component dialect differs:

```ts
// src/routes/blog/[slug]/page.server.ts — same for all four adapters
export async function load({ params }) {
  return { post: await getPost(params.slug) }
}
```

```tsx
// src/routes/blog/[slug]/page.tsx — React
export default function Post({ data }) {
  return <article><h1>{data.post.title}</h1></article>
}
```

See [`examples/smoke-app-{react,vue,svelte,solid}`](examples) for the four-way comparison.

## The CLI

One binary. Grouped by intent:

- **Dev loop** — `oxide dev` `oxide build` `oxide start` `oxide preview`
- **Inspect** — `oxide routes` `oxide analyze` `oxide why` `oxide doctor` `oxide check` `oxide config` `oxide render`
- **Author** — `oxide generate` `oxide add` `oxide migrate` `oxide schema`
- **Operate** — `oxide secrets` `oxide cache` `oxide audit` `oxide test` `oxide mcp`

```
oxide doctor   # 7 env checks: node, cargo, config, routes, cache, secrets, cookie keys
oxide build    # bundles, prerender, typegen, manifests, deploy artifacts
oxide routes   # scan routes, infer render modes, emit typegen, print table
oxide mcp      # expose routes, config, audit, graph as MCP tools for agents
```

## Building from source

```bash
git clone https://github.com/elcoosp/oxide
cd oxide
cargo build --release -p oxide-fw-cli
./target/release/oxide doctor
```

Four end-to-end fixtures ship under [`examples/`](examples):

```bash
oxide build --root examples/smoke-app-react
oxide start --root examples/smoke-app-react --port 3100
```

## Project status

**Pre-alpha. Built against a published specification and audited against it.**

| | |
| --- | --- |
| Spec compliance | **95 / 100** on a 21-section scorecard — [every finding public](docs/deviations.md) |
| CI | 22 jobs on Linux, macOS, Windows — 21 per push, plus nightly Miri |
| Rust | 1,239 test functions; clippy-clean at `-D warnings` with **zero** `#[allow(clippy::*)]` |
| Security | `cargo audit` exits 0; one advisory suppressed with a [reachability analysis](.cargo/audit.toml) |

v0.1.0 is not yet tagged. Releases follow a **48-hour / 5-consecutive-green-CI** soak rule — see [PUBLISHING.md](PUBLISHING.md).

## Documentation

- [`docs/spec-v0.1.md`](docs/spec-v0.1.md) — the specification this release implements
- [`docs/spec-v0.2-draft.md`](docs/spec-v0.2-draft.md) — the working draft for the next release
- [`docs/architecture.md`](docs/architecture.md) — how the crates fit together
- [`docs/deviations.md`](docs/deviations.md) — every known gap between spec and code
- [`docs/plugin-compat.md`](docs/plugin-compat.md) — plugin API surface and Vite/Rollup compatibility
- [`benchmarks/report.md`](benchmarks/report.md) — the perf table with the deterministic-vs-informational split

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) — workflow, local gate list (identical to CI), pnpm config model, soak rule.

## License

MIT OR Apache-2.0 — see [LICENSE](LICENSE).

---

<div align="center">
  <sub>Built against the <a href="docs/spec-v0.1.md">Oxide Technical Specification v0.1</a></sub>
</div>
