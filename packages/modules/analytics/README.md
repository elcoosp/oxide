# @oxide/analytics

The V2-E06 first-party web-vitals module for Oxide. Wires the existing
`@oxide-fw/core` `reportWebVitals` beacon sink (spec §16.6 / §20) with a
generic webhook target and a console target, plus a middleware and a
server-fn contribution the build wires through its existing pipelines.

## Install

```sh
oxide add @oxide/analytics
```

The `oxide add` command detects your package manager (pnpm / yarn / bun /
npm), runs the install, then edits `oxide.config.ts` with an AST splice
(idempotent, comment-preserving) that adds the import + the `modules: []`
array element. A dynamic config is refused loudly with the two lines to
paste.

## Usage

```ts
import { defineConfig } from '@oxide-fw/core'
import analytics from '@oxide/analytics'

export default defineConfig({
  framework: { adapter: 'react' },
  modules: ['@oxide/analytics'],
  plugins: [analytics({ webhook: 'https://example.com/vitals', console: true })],
})
```

## What it contributes

The module's `oxide.module` contract declares (the same shape the Rust
`crates/cli/src/modules_loader.rs` reads):

- `adapters`: `react`, `vue`, `svelte`, `solid` — enforced loudly at `add`
  time and at build time (`MOD-ADAPTER-002`).
- `configDefaults`: merged under `modules.@oxide/analytics` in the
  resolved config (the JSON Schema entry is in
  `crates/config/src/schema.rs`).
- `middlewares`: `./server/middleware.ts` — merged into the route tree's
  middleware index under the module's `/modules/@oxide/analytics/` prefix.
- `serverFns`: `./server/track.ts` — fed to the existing server-fn
  discovery walk (`/_oxide/fn/<id>`).
- `types`: `analytics` — merged into `.oxide/types/analytics.d.ts`.
- `hooks`: `transform` — merged through the plugin chain.

## Tests

```sh
node --test test/*.test.ts
```

Zero runtime deps — `node --test` runs the TS source directly (Node 22+
native type stripping). The recording fake context pins the module's
contributions against the loader's contract.
