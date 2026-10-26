<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-router

**File-based routing: scan, match, layout chains, middleware.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-router.svg)](https://crates.io/crates/oxide-fw-router) [![docs.rs](https://docs.rs/oxide-fw-router/badge.svg)](https://docs.rs/oxide-fw-router)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

Oxide's routes are files. This crate turns `routes/**` into a `RouteTree` that the server can match against in microseconds, with layout chains, nearest error boundaries, and full ancestor middleware resolution.

The routing conventions follow SvelteKit: `routes/blog/[slug]/page.tsx` becomes `/blog/:slug`, `routes/(marketing)/about/page.tsx` becomes `/about` (group elided from the URL, preserved in the layout chain), `routes/@modal/login/page.tsx` becomes a slot. Each file's role (`page`, `layout`, `error`, `not-found`, `template`, `loading`, `route`, `socket`) is inferred from its filename.

## What it provides

- **`RouteTree`** — the compiled tree; `match_path` returns a `MatchResult`
- **`RouteEntry`** — one route: `kind`, `id`, `file`, `segments`, `render_mode`, `has_server_module`, `needs_parent`
- **`RouteKind`** — Page, Layout, Endpoint, Error, NotFound, Template, Loading, Socket
- **`ArchivedTree`** — an mmap'd rkyv archive for zero-alloc route matching in production (built once per `oxide build`)
- **`RulesEngine`** — redirects, rewrites, headers, auth, rate-limits (with token buckets)
- **Segment parsing** — `[slug]`, `[...path]`, `[[opt]]`, `(group)`, `@slot`
- **Prefix indexes** — `middleware_files_for`, `error_route_for`, `not_found_route`

## Usage

```rust
use oxide_fw_router::RouteTree;
use std::path::Path;

let tree = RouteTree::scan(Path::new("src/routes"))?;
if let Some(m) = tree.match_path("/blog/hello-world") {
    println!("route id: {}", m.route.id.0);
    println!("params: {:?}", m.params);
    println!("layouts (outermost first): {:?}", m.layout_chain);
}
```

> **Tip:** In production, load the mmap'd archive instead of scanning: `ArchivedTree::load_archived(path)` reads the prebuilt rkyv archive. The match is a direct memory read — no allocation, no `HashMap` lookups, no per-request parsing. `oxide build` writes this archive to `.oxide/routes.manifest`.

## How it fits in Oxide

`oxide-fw-cli`'s `scan_routes` builds the `RouteTree` from disk at dev/build start. `build_dispatch_swappable` calls `match_path` on every request (or `ArchivedTree::match_path` in production). The matched route's layout chain, error boundary, and middleware files are all precomputed in the tree's prefix indexes — no per-request scans, no `format!` allocations.

The rules engine (`RulesEngine`) is separate from the routing tree: rules are evaluated by `oxide-fw-server`'s pipeline **before** route matching, so a redirect or rate-limit can short-circuit dispatch without ever building a request context.

## See also

- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the HTTP pipeline that uses the tree
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — scans routes and wires the tree
- [`oxide-fw-dev`](https://crates.io/crates/oxide-fw-dev) — re-scans on file change and swaps the tree in dev

---

Full API reference: [docs.rs/oxide-fw-router](https://docs.rs/oxide-fw-router)
