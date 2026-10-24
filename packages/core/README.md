<div align="center">
  <img src="../../docs/logo.png" alt="Oxide" width="140"/>
  <p>
    <strong>The Oxide runtime package.</strong><br/>
    Server data APIs, the ODF wire decoder, the core client router, the
    islands loader, and the ICU runtime — the zero-runtime-dependency
    surface every Oxide app imports from, independent of which framework
    adapter it uses.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Node-24%20type%20stripping-339933?style=flat-square&logo=node.js" alt="Node 24 type stripping"/>
    <img src="https://img.shields.io/badge/TS%20runtime-zero%20npm%20deps-228B22?style=flat-square" alt="Zero runtime deps"/>
    <img src="https://img.shields.io/badge/ODF-wire%20decoder-9D4EDD?style=flat-square" alt="ODF"/>
    <img src="https://img.shields.io/badge/ICU-client%20format-00BFFF?style=flat-square" alt="ICU"/>
    <img src="https://img.shields.io/badge/Tree--shakeable-yes-FF4154?style=flat-square" alt="Tree-shakeable"/>
    <img src="https://img.shields.io/badge/Licence-MIT%20OR%20Apache--2.0-228B22?style=flat-square" alt="Licence"/>
  </p>
  <p>
    <strong><a href="#install">Install</a> · <a href="#entry-points">Entry points</a> · <a href="#server-apis">Server</a> · <a href="#client-runtime">Client</a> · <a href="#odf">ODF</a> · <a href="#licence">Licence</a></strong>
  </p>
</div>

## Install

```bash
pnpm add oxide
```

Zero runtime dependencies. Runs on Node 24 with native TypeScript type
stripping — no build step, no transpiler in the hot path. Every entry
point is an `.ts` file; a bundler (or Node's own stripping) resolves it
from the `exports` map.

## Entry points

The package ships as a set of focused subpath exports. Each is importable
from the adapter packages or a user project without pulling the whole
runtime.

| Subpath | What it provides |
| --- | --- |
| `oxide` | Framework adapter types (`FrameworkAdapter`, `RenderInput`, `HeadTags`) and `defineConfig` |
| `oxide/server` | `defineLoad`, `defineAction`, `defineSocket`, `defineMeta`, `defer`, `cache`, `error`, `redirect`, `getCollection` (§9, §15.2) |
| `oxide/client` | The core router, islands loader, deferred-boundary glue, script loader, i18n runtime, vitals beacon |
| `oxide/client/boot` | The boot script entry (what the build emits into the client bundle) |
| `oxide/client/messages` | ICU formatter (plural / select / selectordinal / number / date / time) |
| `oxide/test` | `makeTestApp` — an in-process `app.fetch` helper for testing a server without a socket |
| `oxide/fn` | Server-function registry primitives (§9.4) |
| `oxide/types` | Shared type surface |

## Server APIs

Everything an app's `*.server.ts` modules import comes from
`oxide/server`. A `defineLoad` returns the route's data; a `defineAction`
is what a form POST invokes; a `defineSocket` is a WebSocket route's
per-phase handler.

```ts
// src/routes/blog/[slug]/page.server.ts
import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async ({ params, defer }) => ({
  title: `Post ${params.slug}`,
  comments: defer(fetchComments(params.slug)),
}))
```

`defer()` hands a promise-like to the pipeline; the boundary that consumes
it streams separately from the shell, so the page is not held back by the
slowest loader. See `<Defer>` in the adapter packages for the client side.

Form actions re-render on a 200 (§9.3), so a cookie set inside a
`defineAction`'s `run` survives onto the rendered response. Redirect
responses pass through unchanged. The `event.cookies` API sets and reads
request-scoped cookies; the session codec (§16.6) is available via
`event.locals.session`.

## Client runtime

`oxide/client` is what the build bundles into the browser. It is not
imported by user code directly; the emitted `__boot__.js` wires it up.
The exports exist so a user can extend the router or read the current
route's data.

- **Router** — intercepts same-origin anchor clicks, fetches `/__data`
  as ODF, updates the DOM in place, and honours `startViewTransition`
  when available.
- **Islands** — hydrates `ox-island` elements per the strategy declared
  in their markup (`load`, `idle`, `visible`, `media(...)`).
- **Deferred boundaries** — attaches the `ox:swap` glue that resolves
  slots the server streams after the shell.
- **ICU** — client-side plural / select / number / date formatting with
  the exact same output the server produces (`Intl` for the numbers,
  the same rules table as the Rust side for the categories).

## ODF

Oxide's wire format (§9.5 / D11) is a compact, devalue-compatible encoder
for loader data, cached responses, and `/__data` payloads. The decoder
lives here; the encoder lives in the Rust core. Both agree byte-for-byte
on the twelve wire records: null, undefined, boolean, number, bigint,
string, date, URL, RegExp, Map, Set, TypedArray, Error, reference,
deferred marker.

`oxide`'s decoder is the client-side reader — it is what `/__data`
responses are parsed into before a route's component sees them.

## Licence

MIT OR Apache-2.0 — same as the rest of Oxide.
