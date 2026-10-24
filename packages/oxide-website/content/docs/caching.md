---
title: Caching
description: Three tiers, content-addressed storage, tags, stale-while-revalidate, single-flight and MAC sealing.
order: 5
---

# Caching

Oxide ships a three-tier cache with semantics usually reserved for CDN architectures — plus a
content-addressed build store underneath.

## The tiers

- **L0 — in-process response cache.** Weak-ETag 304s, stale-while-revalidate, single-flight,
  build seeding, configurable `Vary`. Zero-copy hits over `Arc` entries.
- **L1 — data cache.** `'use cache'` values stored **ODF-encoded**, so `Date`, `Map`, `Set` and
  `BigInt` survive a round trip on both the Node and V8 backends.
- **L2 — pluggable remote handler.** `memory`, `disk`, `redis`, `s3`, or `http` — one interface,
  swap without touching call sites.

## The API surface

```ts
import { cacheTag, cacheLife } from '@oxide-fw/oxide/server'

export const getPost = async (slug: string) => {
  'use cache'
  cacheTag('posts:' + slug)
  cacheLife({ ttl: 60, swr: 300 })
  return { slug, title: `Post ${slug}` }
}
```

The compiler rewrites the module export into a cached function with your tags and lifetime.
Invalidation is tag-based (`invalidateTag`), and page loaders can tag their render output too —
this very site tags blog renders with `blog` and docs with `docs`.

## Correctness details that rarely ship in v0.1

- **Single-flight** — concurrent misses for the same key coalesce; the regression suite locks the
  owner-drop cleanup behavior (no poisoned reservations)
- **Strict MAC sealing** — Redis/S3 payloads are sealed by default ( AEAD
  message-authentication), with tampered-entry rejection as an acceptance test;
  `OXIDE_CACHE_SEALING=legacy` opts out for one migration window
- **Build seeding** — prerendered output seeds the L0 cache, so the first request after deploy is
  already warm

## The build store

The demand-driven incremental graph has early cutoff and a `blake3` content-addressed store on
disk (`redb` index, `zstd` blobs). Byte-stable manifests and typegen mean an unchanged project
hashes identically — per-env client-bundle fingerprints skip rebundling an unchanged env, and
per-route prerender reuse is backed by the persisted graph (`.oxide/graph.json`).
