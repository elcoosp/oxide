---
title: Architecture
description: The pipeline stages with their types and source locations, the request shape, and the crate map.
order: 2
---

# Architecture

Eighteen Rust crates plus seven published TypeScript packages. The design rule:
the request pipeline is ordered so the hot path completes **without entering
JavaScript** whenever the answer is cached, redirected, rewritten or static.

## The request

`OxideRequest` (`crates/server/src/lib.rs:59`) carries the method, path, headers
and body. Every stage below takes it by reference:

```
pub struct OxideRequest {
    pub method: String,
    pub url: String,
    /// P-14: shared across the dispatch clones (loader/action/trace) instead
    /// of a deep `Vec` clone per hop.
    pub headers: std::sync::Arc<Vec<(String, String)>>,
    /// P-14: lazily-built lowercased name → first index, so `header()` is
    /// O(1) after the first lookup instead of a linear scan per call.
    /// Derived header index (P-14). Public only so struct literals in tests
    /// can initialize it; treat it as an implementation detail.
    #[doc(hidden)]
    pub header_index: OnceLock<std::collections::HashMap<Box<str>, usize>>,
    pub body: Bytes,
    pub request_id: String,
    /// §20: the parsed W3C trace context from the incoming `traceparent`
    /// header, when present. Threaded into the fetch op so an outgoing
    /// request carries `traceparent` and a downstream service joins the
    /// same trace. `None` when the header is absent or malformed.
    pub trace: Option<crate::trace_context::TraceContext>,
    /// S-10: the socket peer address. Set by the accept loop (h1/h2 from
    /// `io.peer_addr()`, h3 from `conn.remote_address()`); `None` for a
    /// synthetic/in-process request. Used to derive an unspoofable rate-limit
    /// key instead of trusting `X-Forwarded-For`.
    pub peer: Option<std::net::IpAddr>,
    /// S-20: how many times this request has been re-dispatched by a
    /// middleware rewrite. Internal (never read from an inbound header), so a
    /// client cannot pre-set it to exhaust the rewrite budget.
    pub rewrite_hops: u8,
    /// B-22: the middleware module that issued the rewrite that produced this
    /// request. On re-dispatch the chain skips every layer up to (and
    /// including) that one, so outer middleware does not run twice. Internal,
    /// like `rewrite_hops` — never read from an inbound header.
    pub mw_resume_after: Option<String>,
}

impl Clone for OxideRequest {
    fn clone(&self) -> Self {
        Self {
            method: self.method.clone(),
            url: self.url.clone(),
```

`OxideResponse` (`crates/server/src/lib.rs:187`) has three constructors that
decide the transport shape:

    pub fn html(status: u16, body: impl Into<String>) -> Self   // :245
    pub fn json(status: u16, body: serde_json::Value) -> Self   // :248
    pub fn stream(status: u16, stream: BodyStream) -> Self      // :232

A response built with `stream` leaves `.body` empty; the transport detects the
stream and falls back to chunked encoding. Streaming is the primary live path.

## Pipeline stages

| Stage | Function / type | File:line |
|---|---|---|
| limits | `Limits` | `crates/server/src/lib.rs:263` |
| static scan | `StaticAssets::scan` | `crates/server/src/lib.rs:309` |
| static serve | `StaticAssets::serve` | `crates/server/src/lib.rs:330` |
| asset cache TTL | `asset_cache_ttl` | `crates/server/src/lib.rs:431` |
| negotiation | `negotiate_encoding` | `crates/server/src/lib.rs:593` |
| compression | `compress_body` | `crates/server/src/lib.rs:758` |
| security headers | `security_headers` | `crates/server/src/lib.rs:808` |
| route match | `RouteTree::match_path` | `crates/router/src/lib.rs:139` |
| L0 cache | `CacheHandler::get` | `crates/cache/src/lib.rs:197` |
| middleware | `MiddlewareDispatcher::dispatch` | `crates/server/src/page_renderer.rs:354` |
| loaders | `LoaderRunner::run` | `crates/server/src/loader.rs:150` |
| render | `PageRenderer::render_streaming` | `crates/server/src/page_renderer.rs:130` |
| actions | `ActionDispatcher::dispatch` | `crates/server/src/lib.rs:1732` |

## The four dispatch traits

Every external engine sits behind one of these, so a call site never changes
when an engine does.

`PageRenderer` (`crates/server/src/page_renderer.rs:130`) — `render` (buffered)
and `render_streaming` (chunks) are both adapter-provided. Streaming is the
live path; buffered is what `write_prerender` calls at build time.

`LoaderRunner` (`crates/server/src/loader.rs:150`) — called once per load in the
layout chain. When the page never calls `parent()`, the last layout load and
the page load run under `tokio::join!`. When it does, they run sequentially.

`MiddlewareDispatcher` (`crates/server/src/page_renderer.rs:354`) — outer→inner,
runs before loaders and actions. A result can short-circuit, rewrite the URL, or
contribute `locals`.

`ActionDispatcher` (`crates/server/src/lib.rs:1732`) — called on a form POST
when `?/actionName` is present on a route with a server module.

## Crate map

| Crate | Public entry point |
|---|---|
| `oxide-fw-router` | `RouteTree::scan`, `RouteTree::match_path` (`crates/router/src/lib.rs:139`) |
| `oxide-fw-server` | `Pipeline::serve` + the four traits above |
| `oxide-fw-cache` | `CacheHandler` (`crates/cache/src/lib.rs:196`), `L0Cache` (`tiers.rs`), `TieredCache` (`tiers.rs`) |
| `oxide-fw-odf` | `encode` (`crates/odf/src/lib.rs:171`), `decode` (`:186`), `Value` (`:37`) |
| `oxide-fw-adapters` | `Target` (`:26`), `capability_matrix` (`:90`), `emit` (`:222`) |
| `oxide-fw-compiler` | Oxc passes behind `Transformer` (`crates/compiler/src/oxc_backend.rs`) |
| `oxide-fw-bundler` | `Bundler` trait, rolldown backend (`crates/bundler/src/rolldown_backend.rs`) |
| `oxide-fw-jshost` | `JsHost` / `Pool`, V8 and Node backends (`crates/jshost/src/lib.rs`) |
| `oxide-fw-graph` | `DemandGraph`, content-addressed nodes (`crates/graph/src/lib.rs`) |

## Rendering paths

Real streaming SSR is the primary live path on both backends. Prerendering
runs the full layout chain at build time and seeds the L0 cache, so the first
request after deploy is a hit. The deploy artifact is chosen by adapter and
target — see [Framework adapters](/docs/adapters).
