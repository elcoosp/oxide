# oxide-fw-server

**The HTTP pipeline: hyper transport, security, compression, TLS, HTTP/3.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-server.svg)](https://crates.io/crates/oxide-fw-server) [![docs.rs](https://docs.rs/oxide-fw-server/badge.svg)](https://docs.rs/oxide-fw-server)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The production HTTP server. Takes an `OxideRequest`, runs it through rules → security → CSRF → dispatch → compression → response finalization, and hands it back to hyper.

The pipeline is deliberately framework-agnostic: it knows nothing about React, Vue, or Svelte. It speaks only the trait objects that the CLI wires in — `PageRenderer`, `LoaderRunner`, `ActionDispatcher`, `EndpointDispatcher`, `MiddlewareDispatcher`, `ServerFnDispatcher`, `SocketDispatcher`. Replacing a framework adapter or a caching backend is a one-line change in `oxide.config.ts`, not a pipeline rewrite.

## What it provides

The pipeline:

- **`Pipeline`** — the request handler trait
- **`Server::serve`** / **`serve_with_tls`** / **`serve_http3`** — the three transport entry points
- **`SecuritySettings`** — CSP, CSRF, host allow-list, SSRF allow-list, cookie keys, session keys, trusted proxies
- **`finish_response_with_request`** — response finalization: security headers, hash-mode CSP, CSRF token issuance, `Server-Timing`, compression decision
- **`StaticAssets::scan`** — index a directory by URL path
- **`Metrics`** — per-route latency histogram with cardinality bounded by the route pattern (never the resolved URL)

The dispatch surface (the seven `dyn` traits):

- **`PageRenderer`** — framework adapter entry point
- **`LoaderRunner`** — `page.server.ts` / `layout.server.ts` loaders
- **`ActionDispatcher`** — form-action POST handlers
- **`EndpointDispatcher`** — `route.ts` handlers (SSE, downloads)
- **`MiddlewareDispatcher`** — `middleware.ts` chain
- **`ServerFnDispatcher`** — `/_oxide/fn/<id>` server functions
- **`SocketDispatcher`** — WebSocket `socket.ts` handlers

The endpoints:

- **`ImageEndpoint`** — the framework's on-demand image transform
- **`OgEndpoint`** — SVG-to-raster OG cards
- **`SocketService`** — WebSocket route matching and dispatch

The security primitives:

- **`csrf_check`** / **`csrf_reject`** — origin + double-submit verification (returning `Result<(), Box<Diagnostic>>`)
- **`is_secret_key`** — header-key redaction for logs
- **`nonce_for`** — the per-request CSP nonce

## Usage

```rust
use oxide_fw_server::{Pipeline, SecuritySettings, Server};
use std::sync::Arc;

let pipeline = Arc::new(Pipeline {
    // rule engine, assets, endpoints, dispatchers, security, metrics, ...
    // see the Pipeline struct's full field list on docs.rs
    /* ... */
});
Server::serve(addr, pipeline, dispatch, shutdown_rx).await?;
```

> [!IMPORTANT]
> The dispatch signature is `Fn(OxideRequest) -> BoxedResponse`, where `BoxedResponse` is a boxed `Future<Output = OxideResponse> + Send`. It is not `BoxFuture` from `futures` — this crate does not depend on `futures`, so the pipeline can be built without pulling the trait runtime into a production binary. The type is structurally identical.

## Trait dispatch signatures

Every trait method that takes more than a handful of parameters uses a single struct argument:

```rust
#[async_trait::async_trait]
pub trait EndpointDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        args: EndpointDispatchArgs<'_>,
    ) -> Result<EndpointRunResult, EndpointRunError>;
}

pub struct EndpointDispatchArgs<'a> {
    pub module_path: &'a str,
    pub method: &'a str,
    pub url: &'a str,
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    pub locals: &'a serde_json::Value,
    pub trace: Option<&'a serde_json::Value>,
}
```

This pattern applies to `EndpointDispatcher::dispatch`, `MiddlewareDispatcher::dispatch`, and `ActionDispatcher::dispatch`. It keeps signatures readable, makes adding a field a non-breaking change for callers who use the struct literal, and lets the CLI construct one struct per request instead of packing nine positional arguments.

## How it fits in Oxide

`oxide-fw-cli`'s `cmd_serve` constructs a `Pipeline`, wires the seven `dyn` trait objects to concrete implementations from `oxide-fw-jshost`, and hands it to `Server::serve`. `cmd_dev` does the same but with `hmr: Some(HmrHub)` and the `/__oxide` inspector attached.

Request flow:

1. `from_hyper_request` — parse, strip client-set hop headers, resolve trace context
2. Rules engine — redirect / rewrite / headers / auth / rate-limit / short-circuit
3. CSRF check — origin comparison, double-submit token
4. Dispatch — the dispatch closure (in the CLI) runs middleware → loader → action → render
5. Compression — zstd / br / gzip based on `Accept-Encoding`
6. `finish_response` — security headers, CSP nonce substitution, `Server-Timing`
7. `to_hyper_response` — hand back to hyper

## See also

- [`oxide-fw-router`](https://crates.io/crates/oxide-fw-router) — the route tree the dispatch closure matches against
- [`oxide-fw-render`](https://crates.io/crates/oxide-fw-render) — the HTML stream pipeline the dispatch produces
- [`oxide-fw-cache`](https://crates.io/crates/oxide-fw-cache) — the L0 cache the dispatch reads through
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — wires all dispatchers and starts the server

---

Full API reference: [docs.rs/oxide-fw-server](https://docs.rs/oxide-fw-server)
