---
title: Caching
description: The CacheHandler trait, the Entry freshness window, build_id derivation, and each tier's implementation file.
order: 5
---

# Caching

Three tiers behind one trait. A call site cannot tell which tier answered.

## The trait — `crates/cache/src/lib.rs:196`

    pub trait CacheHandler: Send + Sync {
        async fn get(&self, key: &str) -> Result<Option<Entry>>;             // :197
        async fn set(&self, key: &str, entry: Entry) -> Result<()>;          // :206
        async fn delete(&self, key: &str) -> Result<()>;                     // :207
        async fn tag_keys(&self, tag: &str) -> Result<Vec<String>>;          // :208
        async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize>;  // :209
        async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()>;  // :211
    }

`get_arc` (`:203`) is a provided method returning `Option<Arc<Entry>>` for the
zero-copy L0 path; it calls `get` and wraps.

## The freshness window — `crates/cache/src/lib.rs`

    pub fn is_fresh(&self, now: u64) -> bool    // :103
    pub fn fresh_until(&self) -> u64            // :108
    pub fn stale_until(&self) -> u64            // :115

Those three methods are the whole stale-while-revalidate policy. A response
between `fresh_until` and `stale_until` is served immediately and revalidated
in the background; past `stale_until` it is a miss.

`canonical()` (`:135`) is the MAC-sealed wire form. `canonical_parts` (`:147`)
returns the fields that went into it, so a test can assert the seal without
re-implementing the encoding.

## The key — `crates/cache/src/lib.rs:216`

    pub fn build_id(parts: &[&[u8]]) -> String

Length-prefixed and blake3-based. `["v0.1", route, params]` and
`["v0.1", route, ""]` cannot collide, because the lengths are part of the input.

## The tiers, with their files

| Tier | Type | File |
|---|---|---|
| L0 | `L0Cache` | `crates/cache/src/tiers.rs` |
| L1 | data cache (ODF-encoded values) | `crates/cache/src/tiers.rs`, codec at `crates/cache/src/codec.rs` |
| L2 | `RedisCacheHandler` | `crates/cache/src/redis_handler.rs` |
| L2 | `S3CacheHandler` | `crates/cache/src/s3_handler.rs` |
| L2 | `HttpCacheHandler` | `crates/cache/src/http_handler.rs` |
| compose | `TieredCache` | `crates/cache/src/tiers.rs` |
| single-flight | coalescing | `crates/cache/src/single_flight.rs` |
| URL redaction | log-line auth stripping | `crates/cache/src/auth.rs` |

`TieredCache` composes the three. `single_flight.rs` coalesces concurrent misses
for the same key; its regression suite locks the owner-drop cleanup so a
failed producer cannot leave a poisoned reservation.

## Invalidation

`invalidate_tag(tag, soft)` is on the trait, not above it. The return value is
the number of entries affected. `soft` marks them stale without deleting,
which is what a deploy uses so in-flight readers still see a response.

## What a page writes

Framework-neutral — lives in `page.server.ts`, rewritten by the compiler
(`crates/compiler/src/oxc_backend.rs`):

```ts
import { cacheTag, cacheLife } from '@oxide-fw/core/server'

export const getPost = async (slug: string) => {
  'use cache'
  cacheTag('posts:' + slug)
  cacheLife({ ttl: 60, swr: 300 })
  return { slug, title: `Post ${slug}` }
}
```
