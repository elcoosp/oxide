<div align="center">
  <img src="../../docs/vue.png" alt="Oxide Vue adapter" width="140"/>
  <p>
    <strong>The Vue adapter for Oxide.</strong><br/>
    Real <code>@vue/server-renderer</code> SSR, real <code>createApp</code>
    hydration, and a zero-dependency neutral-IR fallback when Vue is not
    installed. Single-File Components compile at build time through the
    tooling pool.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Vue-3.6-42B883?style=flat-square&logo=vue.js" alt="Vue 3.6"/>
    <img src="https://img.shields.io/badge/Extensions-.vue-42B883?style=flat-square" alt="Extensions .vue"/>
    <img src="https://img.shields.io/badge/Suspense-core-9D4EDD?style=flat-square" alt="Suspense core"/>
    <img src="https://img.shields.io/badge/Islands-yes-00BFFF?style=flat-square" alt="Islands"/>
    <img src="https://img.shields.io/badge/HMR-component-42B883?style=flat-square" alt="HMR component"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero runtime deps"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#quick-start">Quick start</a> · <a href="#capabilities">Capabilities</a> · <a href="#rendering">Rendering</a> · <a href="#islands">Islands</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
pnpm add @oxide/vue vue @vue/server-renderer
```

`vue` and `@vue/server-renderer` are **peer dependencies**
(`^3.6.0-rc.11`). The adapter itself has zero runtime dependencies and runs
on Node 24 type stripping.

## Quick start

```ts
// oxide.config.ts
import vue from '@oxide-fw/vue'
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  framework: vue({ ssr: 'real' }),
  routes: { dir: 'src/routes' },
  adapter: 'binary',
})
```

```vue
<!-- src/routes/page.vue -->
<script setup lang="ts">
defineProps<{ data: { framework: string } }>()
</script>

<template>
  <main>
    <h1>Home</h1>
    <p>Framework: {{ data.framework }}</p>
  </main>
</template>
```

The config carries only the routes directory. SFC components are
discovered from `src/routes` at build time, precompiled through the tooling
pool, and collected into the emitted SSR adapter entry. No config-side
imports, no `componentFor` closure.

`ssr: 'real'` renders through `@vue/server-renderer`;
`createAdapter()` with no options defaults to real SSR (§14.1); `ssr: 'auto'` explicitly prefers real SSR and falls back to the IR renderer when the framework deps are absent; `ssr: 'ir'` selects the IR renderer directly. `ssr: 'real'` requires real SSR and emits a diagnostic instead of a silent IR fallback
to the neutral IR renderer otherwise; `ssr: 'ir'` forces the fallback.

## Capabilities

| Field | Value |
| --- | --- |
| `contract` | `1` |
| `name` | `vue` |
| `extensions` | `['.vue']` |
| `suspense` | `core` (framework-neutral boundary) |
| `islands` | `true` |
| `hmr` | `component` (Vue's own reload) |
| `peerDependencies` | `vue`, `@vue/server-renderer` (`^3.6.0-rc.11`) |

`suspense: 'core'` means the adapter does **not** rely on a Vue-specific
suspense primitive; the core owns the boundary and streams the deferred
content into a framework-agnostic shell. The page's own async setup still
works — the boundary is just not the framework's `<Suspense>`.

## Rendering

`makeRealAdapter` returns a `FrameworkAdapter` whose `render(input)` returns
a `ReadableStream<Uint8Array>` (the frozen §7 contract). Two paths:

1. **Real** — `@vue/server-renderer`'s streaming renderer. The SFC is
   precompiled at build time, imported from the SSR adapter entry, and
   rendered through `renderToStream` (or `renderToString` for the buffered
   shape).
2. **IR** — the neutral element-tree renderer. Same HTML contract, no Vue
   runtime.

A missing page component is a **diagnostic + a 500**, never a silent blank
page. The response carries the `<!--oxide-status:500-->` marker the
pipeline promotes to HTTP 500.

## Islands

```vue
<!-- src/islands/counter.vue -->
<script setup lang="ts">
import { ref } from 'vue'
const props = defineProps<{ initial?: number }>()
const n = ref(props.initial ?? 0)
</script>

<template>
  <button @click="n++">{{ n }}</button>
</template>
```

```vue
<!-- src/routes/page.vue -->
<script setup lang="ts">
import Counter from '../islands/counter.vue'
</script>

<template>
  <Counter :initial="3" />
</template>
```

The adapter emits `<ox-island data-ox-island="Counter" data-ox-chunk="Counter"
data-ox-strategy="load">`; the boot script calls `createApp(...).mount(host)`
on the matching host. Strategies: `load` (default), `idle`, `visible`,
`media(...)`.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
