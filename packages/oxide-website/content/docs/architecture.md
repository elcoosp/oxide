---
title: Architecture
description: The request pipeline, the crate map, and the isolation traits that keep engines swappable.
order: 2
---

# Architecture

Oxide is a workspace of **18 Rust crates** plus **7 published TypeScript packages**
(and 2 private ones: this website and the OG image service). The design rule: the request pipeline is ordered so the hot path completes **without
entering JavaScript** whenever the answer is cached, redirected, rewritten or static.

## The pipeline

```text
request → hyper 1.x → limits + request id → static assets (precompressed, hashed, range, ETag)
        → native rules (redirects · rewrites · headers · basic auth · rate limits)
        → route match (rkyv archive, memory-mapped)
        → L0 response cache (weak-ETag 304s · stale-while-revalidate · single-flight)
        → JS middleware (runs before loaders/actions, may short-circuit or rewrite)
        → loaders: layout chain outer→inner (await parent()); last layout + page load
          run in parallel when the page never calls parent()
        → dispatch: render | action | endpoint (route.ts) | server fn
        → streaming response (real SSR chunks) + Server-Timing + dynamic compression
        → cache fill + tag invalidation
```

## Crate map

- `cli` · `config` · `graph` · `compiler` · `bundler` — the build: typed config extraction, the
  demand-driven incremental graph with early cutoff, and the bundler
- `router` — build-time radix tree serialized to an `rkyv` archive, memory-mapped at boot
- `server` — hyper 1.x + rustls, HTTP/1 + HTTP/2, HTTP/3 over QUIC with automatic `Alt-Svc`
- `render` — streaming SSR orchestration over the adapter contract
- `cache` — the three-tier cache and content-addressed store
- `odf` · `assets` · `dev` · `plugin` · `adapters` · `core-wasm` · `diagnostics` — wire format,
  image/CSS/font engines, dev server, WASM plugins, adapter glue, diagnostics with code frames

## Isolation traits

Every external engine sits behind an isolation trait, so call sites never change when an engine
does:

- `JsHost` — a `deno_core` V8 isolate pool with build-time startup snapshots, or a Node
  child-process pool; both speak the same op protocol
- `Bundler` · `Transformer` · `CssBackend` · `ImageBackend` · `FontBackend` — CSS runs on Lightning
  CSS or an in-house backend; images on a pure-Rust AVIF/WebP/JPEG/PNG pipeline with signed URLs;
  WASM plugins run on a sandboxed `wasmtime` component host with no implicit WASI preopens

## Rendering paths

Real streaming SSR is the primary live render path on both backends: the response streams real
framework chunks as they resolve, with per-settle `defer()` delivery — one nonce'd settle script
per promise as it resolves, the close tag withheld and appended last, a 30s settle bound, and a
kill switch (`OXIDE_DEFER_BUFFERED=1`) that restores batched documents exactly.

Prerendering runs full layout chains at build time (`await parent()` threading, parallel last
layout + page load, real error statuses seeded into the cache), and twelve deploy targets emit
behind a capability matrix that refuses configurations the target cannot honor.
