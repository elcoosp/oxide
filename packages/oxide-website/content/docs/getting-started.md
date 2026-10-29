---
title: Getting started
description: Build the binary, scaffold a route, and write it in all four frameworks — with the code paths each step touches.
order: 1
---

# Getting started

Oxide is a full-stack meta-framework with a Rust core. React, Vue, Svelte and
Solid are adapters over one engine that owns routing, data, caching, bundling,
serving and deployment. You write route files; the framework-neutral work
never reaches your component.

## Prerequisites

- **Rust stable**, edition 2024 (`rust-toolchain.toml` pins the channel)
- **Node ≥ 24** — TypeScript packages run on Node's native type stripping

## 1. Build the binary

    git clone https://github.com/elcoosp/oxide
    cd oxide
    cargo build --release -p oxide-fw-cli
    target/release/oxide doctor

`oxide doctor` is the `Doctor` variant of `Command` in `crates/cli/src/lib.rs:148`.
It runs seven checks — node, cargo, config, routes directory, cache directory,
build secret, cookie keys — and exits non-zero if any fails. Run it first when
something behaves oddly; it catches environment problems before they become
confusing build errors.

## 2. Scaffold a route

    target/release/oxide generate --kind route --root my-app src/routes/blog/[slug]

`Generate` (`crates/cli/src/lib.rs:169`) writes `page.tsx` and, when the route
needs data, `page.server.ts`. Sixteen `create-oxide` templates exist
(4 frameworks × minimal / blog / dashboard / storefront), each shipping a
streaming `defer()` demo.

## 3. One route, four frameworks

The loader lives in `page.server.ts` and returns a plain object. Its signature
is fixed by `LoaderRunner` in `crates/server/src/loader.rs:150`. The component
dialect is the only thing that differs.

```framework-tabs
---react
// src/routes/blog/[slug]/page.server.ts
export async function load({ params }: LoadArgs) {
  return { post: await getPost(params.slug) }
}

// src/routes/blog/[slug]/page.tsx
export default function Post({ data }: { data: { post: Post } }) {
  return <article><h1>{data.post.title}</h1></article>
}
---vue
<script setup lang="ts">
defineProps<{ data: { post: { title: string } } }>()
</script>

<template>
  <article><h1>{{ data.post.title }}</h1></article>
</template>
---svelte
<script lang="ts">
  export let data: { post: { title: string } }
</script>

<article><h1>{data.post.title}</h1></article>
---solid
export default function Post(props: { data: { post: { title: string } } }) {
  return <article><h1>{props.data.post.title}</h1></article>
}
```

Routing, the loader call, the cache key, the streaming behaviour and the
deploy artifact are identical across the four. Only the component file differs.

## 4. The dev loop

    oxide dev       # Dev variant, crates/cli/src/lib.rs:73
    oxide routes    # Routes, :127 — scan, infer modes, emit typegen
    oxide build     # Build, :102 — bundle, prerender, emit artifacts
    oxide start     # Start, :115 — serve; health at /_oxide/health

`oxide build` runs the pipeline in `crates/server/src/lib.rs`: static scan
(`StaticAssets:286`), route match against the `RouteTree` archive
(`crates/router/src/lib.rs:139`), and the L0 cache seed through `CacheHandler`
(`crates/cache/src/lib.rs:196`). The adapter's `emit`
(`crates/adapters/src/lib.rs:222`) writes the deploy artifact.

## 5. Project structure

- `oxide.config.ts` — typed config, statically extracted. `oxide schema`
  (`Command::Schema`, `crates/cli/src/lib.rs:217`) emits a JSON Schema you can
  reference from `$schema` for editor completion.
- `src/routes/` — `page.tsx`, `page.server.ts`, `layout.tsx`, `loading.tsx`,
  `error.tsx`, `not-found.tsx`, `route.ts`, `socket.ts`.
- `public/` — static files, served root-absolute with precompressed variants
  by `StaticAssets::serve` (`crates/server/src/lib.rs:330`).

## Where the pieces live

Reading the source alongside this guide:

| What | File | Symbol |
|---|---|---|
| every subcommand | `crates/cli/src/lib.rs` | `Command` enum, `:73-234` |
| request / response | `crates/server/src/lib.rs` | `OxideRequest:59`, `OxideResponse:187` |
| static assets | `crates/server/src/lib.rs` | `StaticAssets:286`, `serve:330` |
| limits, headers | `crates/server/src/lib.rs` | `Limits:263`, `security_headers:808` |
| route matching | `crates/router/src/lib.rs` | `RouteTree:139`, `MatchResult:128` |
| dispatch traits | `crates/server/src/page_renderer.rs:130,354` | `PageRenderer`, `MiddlewareDispatcher` |
| loader trait | `crates/server/src/loader.rs:150` | `LoaderRunner` |
| action trait | `crates/server/src/lib.rs:1732` | `ActionDispatcher` |
| cache trait | `crates/cache/src/lib.rs:196` | `CacheHandler` — seven methods |
| wire format | `crates/odf/src/lib.rs:171,186` | `encode`, `decode` |
| TS decoder | `packages/core/src/client/odf-decode.ts` | single file, zero deps |
| adapter matrix | `crates/adapters/src/lib.rs:26,90,222` | `Target`, `capability_matrix`, `emit` |

Deeper dives: [Architecture](/docs/architecture),
[Framework adapters](/docs/adapters), [Caching](/docs/caching),
[Wire format (ODF)](/docs/wire-format-odf).
