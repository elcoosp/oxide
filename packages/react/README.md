<div align="center">
  <img src="../../docs/react.png" alt="Oxide React adapter" width="140"/>
  <p>
    <strong>The React adapter for Oxide.</strong><br/>
    Real <code>react-dom/server</code> streaming SSR, real <code>hydrateRoot</code>
    hydration, and a zero-dependency neutral-IR fallback when React is not
    installed. One adapter, two renderers, no config-side component imports.
  </p>
  <p>
    <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react" alt="React 19"/>
    <img src="https://img.shields.io/badge/Extensions-.tsx-3178C6?style=flat-square" alt="Extensions .tsx"/>
    <img src="https://img.shields.io/badge/Suspense-native-9D4EDD?style=flat-square" alt="Suspense native"/>
    <img src="https://img.shields.io/badge/Islands-yes-00BFFF?style=flat-square" alt="Islands"/>
    <img src="https://img.shields.io/badge/HMR-React%20Refresh-FF4154?style=flat-square" alt="HMR Refresh"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero runtime deps"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#quick-start">Quick start</a> · <a href="#capabilities">Capabilities</a> · <a href="#rendering">Rendering</a> · <a href="#islands">Islands</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
pnpm add @oxide/react react react-dom
```

`react` and `react-dom` are **peer dependencies** (`^19.0.0`). The adapter
itself has zero runtime dependencies and runs on Node 24 type stripping.

## Quick start

```ts
// oxide.config.ts
import react from '@oxide/react'
import { defineConfig } from 'oxide'

export default defineConfig({
  framework: react({ ssr: 'real' }),
  routes: { dir: 'src/routes' },
  adapter: 'binary',
})
```

The config carries only the routes directory. Page, layout and
error-boundary components are discovered from `src/routes` at build time
and collected into the emitted SSR adapter entry (`dist/server/ssr/lazy.js`).
No `componentFor` closure, no hand-imported page map — the build owns the
route → component mapping (§6.4).

`ssr: 'real'` renders through `react-dom/server` streaming;
`createAdapter()` with no options defaults to real SSR (§14.1); `ssr: 'auto'` explicitly prefers real SSR and falls back to the IR renderer when the framework deps are absent; `ssr: 'ir'` selects the IR renderer directly. `ssr: 'real'` requires real SSR and emits a diagnostic instead of a silent IR fallback
back to the neutral IR renderer otherwise; `ssr: 'ir'` forces the fallback.

## Capabilities

The adapter manifest declares its capabilities; the core selects the live
path from these.

| Field | Value |
| --- | --- |
| `contract` | `1` |
| `name` | `react` |
| `extensions` | `['.tsx']` |
| `jsxImportSource` | `react` |
| `suspense` | `native` (React `<Suspense>`) |
| `islands` | `true` |
| `hmr` | `refresh` (React Refresh) |
| `peerDependencies` | `react`, `react-dom` (`^19.0.0`) |

`suspense: 'native'` means the core hands the adapter the framework's own
suspense primitive — a boundary in the page renders as a real `<Suspense>`
and the deferred content streams into it, not a framework-agnostic shell.

## Rendering

`makeRealAdapter` returns a `FrameworkAdapter` whose `render(input)` returns
a `ReadableStream<Uint8Array>` (the frozen §7 contract). Two paths, chosen by
`ssr` and by whether `react-dom/server` can be imported:

1. **Real** — `react-dom/server`'s streaming renderer. The page component
   and its layout chain are resolved from the build-emitted component
   surface; `renderStream` (or `renderToPipeableStream`) pumps chunks
   through as they arrive.
2. **IR** — a neutral element-tree renderer. Same HTML contract, no React
   runtime. Used when React is absent, or when `ssr: 'ir'` is set
   explicitly.

Both paths emit the same head merge, the same islands markers, and the same
error-boundary output. The page sees `input.data`, `input.params`,
`input.head`, `input.locale` and `input.layouts` identically.

A missing page component is a **diagnostic + a 500**, never a silent blank
page (§6.4 / §10.2 step 5). The response carries the
`<!--oxide-status:500-->` marker the pipeline promotes to HTTP 500.

## Islands

```tsx
// src/islands/counter.tsx
import { useState } from 'react'

export default function Counter({ initial = 0 }: { initial?: number }) {
  const [n, setN] = useState(initial)
  return <button onClick={() => setN(n + 1)}>{n}</button>
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
data-ox-strategy="load">` and the boot script hydrates it with `hydrateRoot`
on the matching host. Strategies: `load` (default), `idle`, `visible`,
`media(...)`.

## Hydration

The client boot calls `hydrateRoot(container, <Component {...props} />)` on
the server-rendered tree. Navigation swaps call `root.render(<NewComponent
{...newProps} />)` — React updates in place, no remount, no flash.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
