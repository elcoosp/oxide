---
title: Framework adapters
description: React, Vue, Svelte and Solid over one adapter contract — and what an adapter actually owns.
order: 4
---

# Framework adapters

Oxide does not ask you to learn a new component model. Four adapters — React, Vue, Svelte, Solid —
implement one versioned contract (`contract: 1`) over a shared engine. Rendering, hydration and
HMR strategy are the only things an adapter owns; everything else ships identically to every
framework from the core runtime (`packages/oxide`).

## The matrix

| Adapter | Package | Extensions | Suspense | HMR strategy |
| --- | --- | --- | --- | --- |
| React | `@oxide-fw/react` | `.tsx` | native | refresh |
| Vue | `@oxide-fw/vue` | `.vue` | core | component |
| Svelte | `@oxide-fw/svelte` | `.svelte` | core | component |
| Solid | `@oxide-fw/solid` | `.tsx` | native | component |

Each adapter ships a zero-dependency neutral IR renderer **and** a bridge to the real upstream
renderer (`react-dom/server`, `@vue/server-renderer`, `svelte/server`, `@solidjs/web`). Framework
libraries are optional peer dependencies — when absent, the adapter degrades gracefully to the IR
renderer. Real streaming SSR is the primary live render path, and island hydration runs the real
framework runtime on all four.

## The capabilities manifest

A versioned adapter contract defines a capabilities manifest — suspense mode, islands support, HMR
strategy — plus render, hydrate, head, loading and plugins hooks. Releases are gated by
conformance: one reference app (Atlas), one 24-assertion assertion set, all four adapters —
**96 checks per release**, and a release is blocked the moment any adapter drifts. 21 Rust
snapshot tests additionally lock emitted HTML and deploy artifacts byte-for-byte.

## Islands, concretely

Islands are the one interactivity primitive the core owns, so they behave identically everywhere:

```tsx
import { Island } from '@oxide-fw/react'
import Counter from '../islands/counter.tsx'

<Island component="Counter" chunk="Counter" strategy="visible" props={{ initial: 3 }}>
  <Counter initial={initial} />
</Island>
```

- `strategy`: `load` (immediate), `idle` (`requestIdleCallback`), `visible`
  (`IntersectionObserver`), or `{ media: '(...)' }`
- props cross the wire as base64 **ODF** — `Date`, `Map`, `Set`, `BigInt` survive
- the host element is `<ox-island>`; the client boot script scans, decodes and calls the
  adapter's real hydration entry (`createSSRApp` / `mount(hydrate)` / `hydrate` / `createRoot`)

## What the core ships to every adapter

The ODF decoder, the island loader, progressive form `enhance`, script strategies,
deferred-boundary glue, navigation loading boundaries (`onNavigateStart/End`), manifest-driven
chunk preloading on prefetch/goto, and a Core Web Vitals reporter beaming to `/_oxide/vitals` —
zero third-party script.
