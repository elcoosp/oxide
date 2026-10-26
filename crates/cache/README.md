<div align="center">
<img src="https://raw.githubusercontent.com/elcoosp/oxide/main/docs/logo.png" alt="Oxide logo" width="200">
</div>

# oxide-fw-cache

**Three-tier caching with single-flight coalescing and tag invalidation.**

[![Crates.io](https://img.shields.io/crates/v/oxide-fw-cache?style=for-the-badge&logo=rust&label=crates.io)](https://crates.io/crates/oxide-fw-cache) [![Downloads](https://img.shields.io/crates/d/oxide-fw-cache?style=for-the-badge&logo=rust&label=downloads)](https://crates.io/crates/oxide-fw-cache) [![docs.rs](https://img.shields.io/docsrs/oxide-fw-cache?style=for-the-badge&logo=docs.rs)](https://docs.rs/oxide-fw-cache) [![MSRV](https://img.shields.io/badge/MSRV-1.98%2B-blue?style=for-the-badge&logo=rust)](https://github.com/elcoosp/oxide#rust-version)
[![License](https://img.shields.io/crates/l/oxide-fw-cache?style=for-the-badge)](https://github.com/elcoosp/oxide#license) [![CI](https://img.shields.io/github/actions/workflow/status/elcoosp/oxide/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/elcoosp/oxide/actions) [![crate size](https://img.shields.io/crates/size/oxide-fw-cache?style=for-the-badge&label=size)](https://crates.io/crates/oxide-fw-cache)

> **Note:** Part of [Oxide](https://github.com/elcoosp/oxide) — a framework-agnostic, full-stack meta-framework with a Rust core and TypeScript authoring.

## Overview

The layer between a request and a re-render. Oxide's cache is tiered: L0 for full responses (no JS on hit), L1 for loader data (via `'use cache'`), L2 for outbound fetch memoization. Each tier supports memory, disk, Redis, S3, and HTTP backends.

Concurrent misses for the same key run one render — the rest await the result (single-flight). Tag and path invalidation drop entries precisely, not by clearing the whole cache.

## What it provides

- **`CacheHandler` trait** — uniform interface across every backend
- **`TieredCache`** — L0 + L1 + optional remote, sharing one handler
- **`SingleFlight`** — concurrent misses for the same key run one render
- **`CacheKey::canonical_parts`** — stable, prefix-free cache keys
- **Tag and path invalidation** — `invalidate_tag`, `invalidate_path`
- **Stale-while-revalidate and stale-if-error** — configurable policy windows
- **Nonce sentinel handling** — a cached body serves any request without leaking another request's CSP nonce

## Usage

```rust
use oxide_fw_cache::{CacheHandler, TieredCache};
use std::sync::Arc;

let cache: Arc<dyn CacheHandler> = Arc::new(TieredCache::new(512 * 1024 * 1024));
cache.set("route:/blog", entry).await?;
cache.invalidate_tag("route:/blog", false).await?;
```

> **Tip:** Enable the `redis`, `s3`, or `http` features to attach a remote L2 tier. `cache.handler` in `oxide.config.ts` selects which driver to build.

## How it fits in Oxide

`oxide-fw-server`'s pipeline reads through `TieredCache` before rendering. `oxide-fw-cli` wires the same handler instance for L0 (HTTP responses) and L1 (the worker's `'use cache'` entries), so one invalidation triggered by a revalidate action reaches both tiers.

## See also

- [`oxide-fw-server`](https://crates.io/crates/oxide-fw-server) — the pipeline that reads through the cache
- [`oxide-fw-cli`](https://crates.io/crates/oxide-fw-cli) — wires the handler per project config

---

Full API reference: [docs.rs/oxide-fw-cache](https://docs.rs/oxide-fw-cache)
