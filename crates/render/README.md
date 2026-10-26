# oxide-fw-render

**HTML rendering pipeline: streaming, boundaries, injection.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-render.svg)](https://crates.io/crates/oxide-fw-render) [![docs.rs](https://docs.rs/oxide-fw-render/badge.svg)](https://docs.rs/oxide-fw-render)

> [!NOTE]
> Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The Rust side of the render pipeline, after the framework adapter has produced HTML. Everything here operates on the **HTML stream**, not on the component tree — which is what makes streaming SSR fast and framework-agnostic: the pipeline does not care whether it's React's `renderToPipeableStream`, Vue's `renderToNodeStream`, or Svelte's output.

Two problems this crate solves that a naive pipeline cannot:

1. **Mid-render error recovery.** If a render errors after the shell has been flushed, truncating the response shows the browser a half-page. This crate appends a swappable error boundary instead, keeps the status at 200, and lets the client's inline script restore the boundary.
2. **Injection that does not buffer.** The `</body>` splice is common — but it has to survive a chunk boundary that splits `</body>` across two flushes. A rolling tail keeps at most `len("</body>") - 1` bytes held, so no other chunk is delayed.

## What it provides

- **Boundary marker protocol** — `data-ox-b` placeholder, `data-ox-c` content template, `data-ox-error` recovery boundary
- **`inject_before_close`** — splice bytes before `</body>`, chunk-split safe
- **`hydration_bootstrap`** — the `window.__oxide` script tag (ODF payload embedded)
- **`render_stream_with_recovery`** — mid-stream error recovery
- **`RequestTimings::server_timing()`** — the `Server-Timing` header string
- **`ppr::split_shell`** — extract a PPR shell from a completed document
- **`stream_error_recovery`** — the inline error boundary script

## Usage

```rust
use oxide_fw_render::{inject_before_close, RequestTimings};

let timings = RequestTimings {
    middleware_ms: 1.2,
    cache_ms: 0.0,
    load_ms: 12.5,
    render_ms: 45.1,
    inject_ms: 0.3,
};
let tag = format!(r#"<script type="module" src="/client/main.js"></script>"#);
let body = inject_before_close(stream, b"</body>", tag.as_bytes());
println!("{}", timings.server_timing());
// "middleware;dur=1.2, cache;dur=0, load;dur=12.5, render;dur=45.1, inject;dur=0.3"
```

> [!IMPORTANT]
> A response is never truncated mid-stream. If the adapter fails after the shell commits, `render_stream_with_recovery` emits a `data-ox-error` boundary with the diagnostic code and message embedded, closes the stream cleanly, and leaves the status at 200. The client's inline script (also injected here) swaps the error into the matching placeholder. A render error therefore produces a *recoverable page*, not a broken connection.

## How it fits in Oxide

`oxide-fw-cli`'s `build_dispatch_swappable` calls `render_stream_with_recovery` around the adapter's output stream, then `inject_before_close` to splice in:
- the hydration bootstrap (`window.__oxide` ODF payload)
- the deferred-settle `<script>` tags
- the boot `<script type="module">`
- the dev HMR client

`oxide-fw-server` uses `RequestTimings::server_timing()` to fill the `Server-Timing` header in `finish_response_with_request`. The `ppr` submodule is used by the CLI when `render.ppr: true`.

## See also

- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the HTTP pipeline that consumes the body stream
- [`oxide-fw-odf`](https://crates.io/crates/oxide-fw-odf) — the hydration payload format
- [`oxide-fw-adapters`](https://crates.io/crates/oxide-fw-adapters) — produces the initial HTML stream

---

Full API reference: [docs.rs/oxide-fw-render](https://docs.rs/oxide-fw-render)
