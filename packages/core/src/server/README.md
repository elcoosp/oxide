# Real-adapter contract (spec §7) — what the Rust CLI needs to consume

This directory owns the JS-side surface the Rust build pipeline consumes to
select between the neutral-IR renderer (which drives the §22 conformance
suite and runs with zero framework deps) and the real upstream renderer
(`react-dom/server`, `@vue/server-renderer`, `svelte/server`, `@solidjs/web`).

## What each adapter package exports

Every `@oxide-fw/react`, `@oxide-fw/vue`, `@oxide-fw/svelte`, `@oxide-fw/solid` exports:

| Export | Shape | Purpose |
| :--- | :--- | :--- |
| `adapter` | `FrameworkAdapter` | The neutral-IR renderer. Byte-stable, no framework deps, drives §22. |
| `createAdapter(opts)` | `(AdapterOptions) => FrameworkAdapter` | Factory. Selects the real renderer for `ssr: 'real' \| 'auto'` (or an explicit `componentFor` override); components come from the runtime resolver. With no options it returns `adapter`. |
| `createRealAdapter(opts)` | `(RealAdapterOptions) => FrameworkAdapter` | Lower-level factory over `makeRealAdapter`; exposed for tests and custom hosts. |
| *default* export | `(opts?) => FrameworkAdapter` | Callable alias for `createAdapter`. Config surface. |

## The config surface (spec §14)

`oxide.config.ts` reads:

```ts
import react from '@oxide-fw/react'
export default defineConfig({
  framework: react({ ssr: 'real' }),
  routes: { dir: 'src/routes' },
})
```

The config is the §14.1 literal shape: no page/layout/error imports and no
`componentFor` closure. Route components are collected from `routes.dir` at
build time. `componentFor`/`layoutFor` survive only as explicit advanced
overrides (custom hosts, tests); the generated resolver wins by default.

`framework` receives whatever the default export returns — a
`FrameworkAdapter` (contract 1). The Rust CLI at build time must:

1. **Load** the config module from a V8 context (spec §4.2, `JsHost`).
2. **Extract** the `framework` value. It is a plain object with the
   `FrameworkAdapter` shape (§7): `name`, `extensions`, `capabilities`,
   `render(input)`, `head(ctx)`, `hydrate`, `hydrateIsland?`, `mount`,
   `update`.
3. **Route** the request pipeline:
   - `adapter.render(input)` returns a `ReadableStream<Uint8Array>` — the
     Rust pipeline pumps it into the HTTP response, injecting CSP nonces,
     compression, and headers (§10.2, §12.1).
   - `adapter.head(ctx)` returns the neutral `HeadTags` model; the Rust
     `render::HeadTags::merge` merges it with layout head (§16.3).
4. **Dispatch client** hydration through the adapter's `hydrate` /
   `hydrateIsland` / `mount` / `update` exports (bundled into the client
   entry, spec §6.4).

## Where components come from (spec §6.4)

The build emits `dist/server/ssr/index.js` with
`export const routes = { "<canonical id>": moduleNamespace }`. The worker
imports it once per module generation and builds the resolver the adapter
sees as `input.resolve(id)`:

- **prod:** the generated entry (one import per generation — never the config
  graph per render);
- **dev:** the route-table → source-file map, imported through the tooling
  loader so `.tsx`/`.vue`/`.svelte` recompile on HMR.

Canonical ids are the directory form (`routes/blog/[slug]`), no `/page`
suffix. The resolver unwraps the module namespace (`default`, `Page`, …).
A missing **page** is the §10.2 `OX-500` diagnostic (HTTP 500), never a
silent blank body; a missing layout/error logs a diagnostic.

## What the CLI does NOT need to know

- **Which framework a route uses.** The runtime resolver owns id → component;
  the adapter asks the input, not the config.
- **IR vs real dispatch.** The adapter decides internally based on `opts.ssr`
  and whether the framework deps are installed.
- **Escaping, streaming, boundary protocol.** All of that lives behind the
  `FrameworkAdapter` interface and is byte-identical between the IR and real
  paths (the §22 conformance suite asserts this).

## Determinism guarantee

The IR renderer emits byte-identical output across runs and across
machines (FNV-1a scope ids, sorted attribute emission). This is what makes
hydration mismatch-free (§7.2). The real renderer defers that guarantee to
the upstream framework — React, Vue, Svelte, and Solid all emit
deterministic SSR markup given the same input tree, which is what the
per-adapter `real-ssr.test.ts` files verify.

## Rust-side status

Wired end to end: `oxide build` emits the entry + manifests, `oxide start`
dispatches renders with the entry-derived resolver, `oxide dev` supplies the
source map, and the V8 and Node backends both consume it. See
`crates/cli/src/framework_shim.rs` (`dev_component_sources`,
`LazyFrameworkRenderer::with_components`) and
`crates/jshost/src/framework_renderer.rs` (`ComponentSource`).
