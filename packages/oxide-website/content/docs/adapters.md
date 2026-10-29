---
title: Framework adapters
description: The Target enum, the capability matrix that refuses impossible configurations, and the same route in four dialects.
order: 4
---

# Framework adapters

An adapter owns rendering, hydration and HMR strategy. Routing, loaders, the
cache, the pipeline and the deploy artifact are core and behave identically.

## The twelve targets

`Target` (crates/adapters/src/lib.rs:26) is the enum of deploy targets — Binary, Node,
Static, Docker, Cloudflare, CloudflarePages, Lambda, Vercel, VercelFunctions,
Netlify, Deno, Bun. `Target::parse` (crates/adapters/src/lib.rs:42) maps a config string to a
variant; `Target::as_str` (crates/adapters/src/lib.rs:59) is the inverse, used when writing
the artifact manifest.

## The capability matrix

Three types, all in `crates/adapters/src/lib.rs`:

- `Capability` (crates/adapters/src/lib.rs:79) — what a target can or cannot do
- `FeatureSupport` (crates/adapters/src/lib.rs:90) — `Full`, `Partial(reason)`,
  `None(reason)`
- `BuildArtifact` (crates/adapters/src/lib.rs:203) — the build id, routes, assets, rules

`capability_matrix` (crates/adapters/src/lib.rs:90) returns a
`BTreeMap<Capability, FeatureSupport>` — deterministic order, so the emitted
manifest is byte-stable. `degradation` (crates/adapters/src/lib.rs:152) returns the
one-line reason a capability is unavailable on a target, which is what the
build error quotes when a configuration is refused.

`emit` (crates/adapters/src/lib.rs:222) is the entry point. It dispatches per target:

| Target | Emitter | Location |
|---|---|---|
| static | `emit_headers` | crates/adapters/src/lib.rs:326 |
| static | `emit_redirects` | crates/adapters/src/lib.rs:827 |
| docker | `dockerfile` | crates/adapters/src/lib.rs:237 |
| cloudflare | `cloudflare_worker_entry` | crates/adapters/src/lib.rs:345 |
| cloudflare | `cloudflare_wrangler_toml` | crates/adapters/src/lib.rs:375 |
| cloudflare-pages | `cloudflare_pages_wrangler_toml` | crates/adapters/src/lib.rs:423 |
| deno | `deno_deploy_config` | crates/adapters/src/lib.rs:543 |
| vercel-functions | `vercel_functions_json` | crates/adapters/src/lib.rs:456 |
| node | `node_server_entry` | crates/adapters/src/lib.rs:255 |
| edge (wasm) | `edge_handler_js` | crates/adapters/src/lib.rs:349 |

`emit` returns the paths it wrote, so a caller can log them or a test can
assert the artifact shape without walking the directory.

## Where the matrix is tested

`crates/adapters/src/tests.rs` asserts, for every `Target` × `Capability`
pair, that `capability_matrix` returns the expected `FeatureSupport`. Adding
a `Capability` without a corresponding test entry fails the build — that is
what keeps the matrix honest as targets are added. `degradation` is tested
with the same enumeration, so a capability that is silently `None` on every
target is caught.

The test file also pins the byte output of each emitter above: one test per
target that writes the artifact to a tempdir and compares against a committed
fixture. The fixture files live under `crates/adapters/tests/fixtures/`, so a
change to `cloudflare_worker_entry` or `dockerfile` fails a specific test
rather than producing a subtly different deploy.

## Same route, four dialects

The loader is one file and one signature — `LoaderRunner` at
`crates/server/src/loader.rs:150`. The component is the framework.

```framework-tabs
---react
// page.server.ts — shared by all four
export async function load({ params }: LoadArgs) {
  return { post: await getPost(params.slug) }
}

// page.tsx
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

Each adapter ships a zero-dependency neutral IR renderer and a bridge to the
real upstream renderer. Framework libraries are optional peer dependencies:
absent, the adapter renders through the IR; present, through the real
runtime. The render path is `PageRenderer` at
`crates/server/src/page_renderer.rs:130`.

## Islands

The one interactivity primitive the core owns. `strategy` decides *when* to
hydrate; the adapter decides *how*.

    <Island component="Counter" chunk="Counter" strategy="visible" props={{ initial: 3 }}>
      <Counter initial={3} />
    </Island>

Strategies: `load`, `idle`, `visible`, or `{ media: '...' }`. Props are base64
ODF — see [the wire format](/docs/wire-format-odf). The island wrapper is
emitted as `<ox-island>` by the render pipeline at
`crates/server/src/lib.rs:187` (`OxideResponse`).
