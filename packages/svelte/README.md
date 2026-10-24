<div align="center">
  <img src="../../docs/svelte.png" alt="Oxide Svelte adapter" width="140"/>
  <p>
    <strong>The Svelte adapter for Oxide.</strong><br/>
    Real Svelte 5 SSR, real hydration, and a zero-dependency neutral-IR
    fallback when Svelte is not installed. <code>.svelte</code> components
    compile at build time through the tooling pool.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Svelte-5-FF3E00?style=flat-square&logo=svelte" alt="Svelte 5"/>
    <img src="https://img.shields.io/badge/Extensions-.svelte-FF3E00?style=flat-square" alt="Extensions .svelte"/>
    <img src="https://img.shields.io/badge/Suspense-core-9D4EDD?style=flat-square" alt="Suspense core"/>
    <img src="https://img.shields.io/badge/Islands-yes-00BFFF?style=flat-square" alt="Islands"/>
    <img src="https://img.shields.io/badge/HMR-component-FF3E00?style=flat-square" alt="HMR component"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero runtime deps"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#quick-start">Quick start</a> · <a href="#capabilities">Capabilities</a> · <a href="#rendering">Rendering</a> · <a href="#islands">Islands</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
pnpm add @oxide/svelte svelte
```

`svelte` is a **peer dependency** (`^5.0.0`). The adapter itself has zero
runtime dependencies and runs on Node 24 type stripping.

## Quick start

```ts
// oxide.config.ts
import svelte from '@oxide-fw/svelte'
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  framework: svelte({ ssr: 'real' }),
  routes: { dir: 'src/routes' },
  adapter: 'binary',
})
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  export let data: { framework: string }
</script>

<main>
  <h1>Home</h1>
  <p>Framework: {data.framework}</p>
</main>
```

The config carries only the routes directory. `.svelte` components are
discovered from `src/routes` at build time, precompiled through the tooling
pool, and collected into the emitted SSR adapter entry. No config-side
imports.

`ssr: 'real'` renders through Svelte 5's server API;
`createAdapter()` with no options defaults to real SSR (§14.1); `ssr: 'auto'` explicitly prefers real SSR and falls back to the IR renderer when the framework deps are absent; `ssr: 'ir'` selects the IR renderer directly. `ssr: 'real'` requires real SSR and emits a diagnostic instead of a silent IR fallback
to the neutral IR renderer otherwise; `ssr: 'ir'` forces the fallback.

## Capabilities

| Field | Value |
| --- | --- |
| `contract` | `1` |
| `name` | `svelte` |
| `extensions` | `['.svelte']` |
| `suspense` | `core` (framework-neutral boundary) |
| `islands` | `true` |
| `hmr` | `component` (Svelte's own reload) |
| `peerDependencies` | `svelte` (`^5.0.0`) |

`suspense: 'core'` means the boundary is owned by the core, not by a
Svelte-specific primitive. The page's own async work still streams into a
framework-agnostic shell.

## Rendering

`makeRealAdapter` returns a `FrameworkAdapter` whose `render(input)` returns
a `ReadableStream<Uint8Array>` (the frozen §7 contract). Two paths:

1. **Real** — Svelte 5's `render()` from `svelte/server`. The `.svelte`
   component is precompiled at build time, imported from the SSR adapter
   entry, and rendered with the page's data as props.
2. **IR** — the neutral element-tree renderer. Same HTML contract, no
   Svelte runtime.

Svelte 5 SSR is buffer-oriented (`renderToString`-shaped), so the adapter's
`renderStream` is a single-chunk stream over the buffered output. Streaming
boundaries are a Svelte 6 feature; the core's defer path still flushes
settled values into the page as they resolve.

A missing page component is a **diagnostic + a 500**, never a silent blank
page. The response carries the `<!--oxide-status:500-->` marker the
pipeline promotes to HTTP 500.

## Islands

```svelte
<!-- src/islands/counter.svelte -->
<script lang="ts">
  export let initial = 0
  let n = initial
</script>

<button on:click={() => n++}>{n}</button>
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import Counter from '../islands/counter.svelte'
</script>

<Counter initial={3} />
```

The adapter emits `<ox-island data-ox-island="Counter" data-ox-chunk="Counter"
data-ox-strategy="load">`; the boot script mounts the compiled component on
the matching host. Strategies: `load` (default), `idle`, `visible`,
`media(...)`.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
