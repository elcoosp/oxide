---
title: Introducing Oxide — one Rust core, every framework
description: A full-stack meta-framework with a Rust core — real streaming SSR for React, Vue, Svelte and Solid, from a single static binary. What it is, why it exists, and exactly how far along it is.
date: 2026-10-26
tags: [oxide, rust, webdev, ssr, announcement]
---

Every few months someone announces a JavaScript meta-framework, and the announcement quietly
assumes the same trade you made years ago: your components, your routing, your caching and your
server all live in one framework's opinion of the world. Want React at work and Svelte on the
side? Two stacks. Want the server to stop booting Node for a redirect? Too bad — the config file
alone evaluates JavaScript before your app does.

**Oxide** takes the opposite bet: **one Rust core, every framework.** React, Vue, Svelte and Solid
are first-class adapters over a shared engine that owns routing, data, caching, bundling, serving
and deployment — so anything framework-neutral behaves identically on all four. You keep the
component model you know; the framework stops being your runtime.

This post is the honest introduction: what it is, what is real today, and what is not.

## The four pillars

**1. The hot path skips JavaScript.** The route table is a build-time radix tree serialized to an
`rkyv` archive and memory-mapped at boot — matches read the mapped bytes in place. Redirects,
rewrites, headers, basic auth and rate limits evaluate natively in Rust. The typed config is
statically extracted from `oxide.config.ts`; the config path itself never evaluates JavaScript. A
cached response is served from an in-process L0 cache without a single JS frame.

**2. Real streaming SSR.** When a page does need JavaScript, you get real framework output —
`react-dom/server`, `@vue/server-renderer`, `svelte/server`, `@solidjs/web` — streamed chunk by
chunk as it resolves. Deferred values settle into the stream one nonce'd script per promise, the
document's close tag withheld and appended last. Islands hydrate the real framework runtime on all
four adapters, with `load` / `idle` / `visible` / `media()` strategies.

**3. One wire format.** ODF, the Oxide Data Format, is a binary format encoded natively in Rust
and decoded by a single-file, zero-dependency TypeScript module. The same bytes go to the cache
and to the wire — so a `Date`, `Map`, `Set` or `BigInt` that enters the cache comes out intact.
Sixteen golden fixtures lock the format byte-exactly on both sides. More on that in the
[next post](/blog/one-wire-format-two-languages).

**4. A single binary.** hyper 1.x + rustls: HTTP/1 and HTTP/2, with HTTP/3 over QUIC and automatic
`Alt-Svc` advertisement. Precompressed static serving (zstd › brotli › gzip), range requests,
streaming responses with dynamic compression and `Server-Timing`. Twelve deploy targets — binary,
Node, Docker, Cloudflare, Lambda, Vercel, Netlify, Deno, Bun and friends — behind a capability
matrix that **refuses at build time** any configuration the target cannot honor, rather than
failing at runtime.

## Why not just use X?

Fair question, and the answer matters:

- **vs Next.js / Nuxt / SvelteKit** — those are excellent and tied to one framework each. Oxide is
  the layer *under* the framework: pick your adapter, keep everything else identical. It even
  ships `oxide migrate` codemods for Next, Nuxt, SvelteKit and Vite apps.
- **vs Leptos / Dioxus / full-Rust stacks** — those replace your components with Rust. Oxide
  deliberately does not: your `.tsx` / `.vue` / `.svelte` files stay exactly as they are. Rust owns
  the server, not the UI tree.
- **vs rolling your own** — the engine part is the boring, error-prone part: incremental builds,
  cache semantics, HMR, streaming protocols, deploy targets. Oxide is that layer, with tests.

## The part we're proudest of

The numbers, because they're checkable:

- **1,116 Rust test functions** across 17 crates — including `proptest` suites on the router and
  graph engines and `cargo-fuzz` harnesses on the ODF decoder
- **349 TypeScript test cases**, all green, zero runtime npm dependencies
- **96 conformance checks per release**: one reference app, one 24-assertion suite, all four
  adapters — a release is blocked the moment any adapter drifts
- **A public compliance scorecard**: we wrote a 21-section specification and audit ourselves
  against it every round — currently **91 / 100**, findings public, [including the ugly
  ones](/blog/auditing-our-own-framework-91-of-100)

## Try it

```sh
git clone https://github.com/elcoosp/oxide
cd oxide
cargo build --release -p oxide-cli

node packages/create-oxide/src/cli.ts blog react my-app   # 16 templates: 4 frameworks × 4 shapes
```

This very site is rendered by Oxide — streaming SSR, one island, OG image cards minted by
`/_oxide/og`, RSS from a `route.ts` endpoint, deployed as one binary.

## Honest status

Oxide is **pre-alpha**, built against a published specification, and the gaps are documented in
public: partial prerendering's shell splitter is tested but not wired into a render path yet; the
edge build emits a WASM blob that no consumer chain reads yet; socket fan-out currently reaches
only the calling socket. The full list lives in the repo README and HANDOFF — we update it every
audit round, and we don't market the list's contents as features.

If you want production-hardened, wait six months. If you want to watch a framework grow with its
test suite open — or claim some of the ~40 open work packages — [the repo is
here](https://github.com/elcoosp/oxide). The docs start at [Getting
started](/docs/getting-started), and the RSS feed is at [/rss](/rss).
