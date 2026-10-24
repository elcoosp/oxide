<div align="center">
  <img src="../../docs/solid.png" alt="Oxide Solid adapter" width="140"/>
  <p>
    <strong>The Solid adapter for Oxide.</strong><br/>
    Real <code>@solidjs/web</code> SSR with native suspense, real
    <code>hydrate</code>, and a zero-dependency neutral-IR fallback when
    Solid is not installed. <code>ssrElement</code> is the streaming
    primitive.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Solid-2.0-2C4F7C?style=flat-square&logo=solid" alt="Solid 2.0"/>
    <img src="https://img.shields.io/badge/Extensions-.tsx-3178C6?style=flat-square" alt="Extensions .tsx"/>
    <img src="https://img.shields.io/badge/Suspense-native-9D4EDD?style=flat-square" alt="Suspense native"/>
    <img src="https://img.shields.io/badge/Islands-yes-00BFFF?style=flat-square" alt="Islands"/>
    <img src="https://img.shields.io/badge/HMR-component-2C4F7C?style=flat-square" alt="HMR component"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero runtime deps"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#quick-start">Quick start</a> · <a href="#capabilities">Capabilities</a> · <a href="#rendering">Rendering</a> · <a href="#islands">Islands</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
pnpm add @oxide/solid solid-js @solidjs/web
```

`solid-js` and `@solidjs/web` are **peer dependencies**
(`^2.0.0-rc.14`). The adapter itself has zero runtime dependencies and runs
on Node 24 type stripping.

> **Solid 2 note:** `solid-js/web` no longer exists as a subpath — the
> surface moved to the `@solidjs/web` package. The adapter's
> `jsxImportSource` reflects that. A project pinned to Solid 1.x should stay
> on the adapter's `0.x` line.

## Quick start

```ts
// oxide.config.ts
import solid from '@oxide-fw/solid'
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  framework: solid({ ssr: 'real' }),
  routes: { dir: 'src/routes' },
  adapter: 'binary',
})
```

```tsx
// src/routes/page.tsx
export default function Page(props: { data: { framework: string } }) {
  return (
    <main>
      <h1>Home</h1>
      <p>Framework: {props.data.framework}</p>
    </main>
  )
}
```

The config carries only the routes directory. `.tsx` components are
discovered from `src/routes` at build time and collected into the emitted
SSR adapter entry. No config-side imports, no `componentFor` closure.

`ssr: 'real'` renders through `@solidjs/web`'s `ssrElement`;
`createAdapter()` with no options defaults to real SSR (§14.1); `ssr: 'auto'` explicitly prefers real SSR and falls back to the IR renderer when the framework deps are absent; `ssr: 'ir'` selects the IR renderer directly. `ssr: 'real'` requires real SSR and emits a diagnostic instead of a silent IR fallback
to the neutral IR renderer otherwise; `ssr: 'ir'` forces the fallback.

## Capabilities

| Field | Value |
| --- | --- |
| `contract` | `1` |
| `name` | `solid` |
| `extensions` | `['.tsx']` |
| `jsxImportSource` | `@solidjs/web` |
| `suspense` | `native` (Solid `<Suspense>`) |
| `islands` | `true` |
| `hmr` | `component` (Solid's own reload) |
| `peerDependencies` | `solid-js`, `@solidjs/web` (`^2.0.0-rc.14`) |

`suspense: 'native'` means the boundary is Solid's own `<Suspense>`, so a
deferred slot streams into a real Solid boundary — reactive, not a
framework-agnostic shell.

## Rendering

`makeRealAdapter` returns a `FrameworkAdapter` whose `render(input)` returns
a `ReadableStream<Uint8Array>` (the frozen §7 contract). Two paths:

1. **Real** — `@solidjs/web`'s `ssrElement`. The page component and its
   layout chain are resolved from the build-emitted component surface and
   rendered through `ssrElement`, which streams into the response.
2. **IR** — the neutral element-tree renderer. Same HTML contract, no Solid
   runtime.

A missing page component is a **diagnostic + a 500**, never a silent blank
page. The response carries the `<!--oxide-status:500-->` marker the
pipeline promotes to HTTP 500.

**Documented gap:** the `<Image>` component's `priority` preload hint is a
no-op on Solid — `ssrElement` returns one descriptor, and a two-element
fragment (preload link + image) needs a wrapper. Tracked in
`docs/deviations.md`; the image itself renders correctly.

## Islands

```tsx
// src/islands/counter.tsx
import { createSignal } from 'solid-js'

export default function Counter(props: { initial?: number }) {
  const [n, setN] = createSignal(props.initial ?? 0)
  return <button onClick={() => setN(n() + 1)}>{n()}</button>
}
```

```tsx
// src/routes/page.tsx
import Counter from '../islands/counter'

export default function Page() {
  return <Counter initial={3} />
}
```

The adapter emits `<ox-island data-ox-island="Counter" data-ox-chunk="Counter"
data-ox-strategy="load">`; the boot script calls Solid's `hydrate` on the
matching host. Strategies: `load` (default), `idle`, `visible`,
`media(...)`.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
