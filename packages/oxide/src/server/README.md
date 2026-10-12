# Real-adapter contract (spec §7) — what the Rust CLI needs to consume

This directory owns the JS-side surface the Rust build pipeline consumes to
select between the neutral-IR renderer (which drives the §22 conformance
suite and runs with zero framework deps) and the real upstream renderer
(`react-dom/server`, `@vue/server-renderer`, `svelte/server`, `@solidjs/web`).

## What each adapter package exports

Every `@oxide/react`, `@oxide/vue`, `@oxide/svelte`, `@oxide/solid` exports:

| Export | Shape | Purpose |
| :--- | :--- | :--- |
| `adapter` | `FrameworkAdapter` | The neutral-IR renderer. Byte-stable, no framework deps, drives §22. |
| `createAdapter(opts)` | `(AdapterOptions) => FrameworkAdapter` | Factory. With `componentFor`, dispatches to the real renderer for resolved routes. Without, returns `adapter`. |
| `createRealAdapter(opts)` | `(RealAdapterOptions) => FrameworkAdapter` | Lower-level factory: requires `componentFor` at construction; exposed for tests. |
| *default* export | `(opts?) => FrameworkAdapter` | Callable alias for `createAdapter`. Config surface. |

## The config surface (spec §14)

`oxide.config.ts` reads:

```ts
import react from '@oxide/react'
export default defineConfig({
  framework: react({ compiler: true, ssr: 'auto', componentFor: (route) => routes[route]?.component }),
  // …
})
```

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

## What the CLI does NOT need to know

- **Which framework a route uses.** `componentFor(route)` is the caller's
  responsibility; the adapter asks the caller, not the reverse.
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

## What remains (Rust side)

The Rust CLI does not yet load `oxide.config.ts` at build time. Until it
does:

- `oxide dev` / `oxide build` / `oxide start` continue to work against the
  neutral-IR renderer via the existing `crates/render`, `crates/server`, and
  `crates/jshost` wiring.
- The real-adapter path is fully exercised by JS tests
  (`packages/*/test/real-adapter.test.ts`, `real-ssr.test.ts`,
  `adapter-factory.test.ts`) and by the §22 conformance suite, which
  imports the named `adapter` export and would continue to do so.
- Wiring is a §6.3 / §7 pipeline change: the CLI must call `JsHost::call`
  with the config's `framework` value on a real V8 pool, then route the
  returned `FrameworkAdapter` through the existing pipeline.
