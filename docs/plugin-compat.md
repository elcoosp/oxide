# Plugin compatibility (SPEC-15.3)

Oxide plugins come in three kinds (SPEC §15.1). This table documents which
hooks each kind supports and whether the support is wired end-to-end
(load → invoke → result consumed) or only partially implemented.

**Status legend:**

- ✅ **wired** — the hook is invoked by the pipeline and its result is used
- 🟡 **partial** — the enum/type exists and the host can call it, but some
  call site or argument shape is still missing
- 🚫 **unsupported** — declaring it at load time triggers `OX-PLUGIN-HOOK`
- n/a — the kind does not expose this hook

## Kinds

| Kind | What it is | Host |
|---|---|---|
| `js` | A Node ESM module exporting hook functions; runs in the tooling pool. | `plugin:load` / `plugin:call` (Node and V8 backends) |
| `wasm` | A `wasmtime` component satisfying `oxide:plugin/hooks@0.1.0`. | `oxide_plugin::WasmPlugin` |
| `native` | A built-in implementation (images, fonts, i18n, mdx). | Rust crate |

## Hooks

| Hook | § | `js` | `wasm` | `native` | Notes |
|---|---|---|---|---|---|
| `config` | 15.2 | ✅ | 🚫 | n/a | Load-time patch, merged before the build. |
| `configResolved` | 15.2 | ✅ | 🚫 | n/a | Read-only view after the config is frozen. |
| `buildStart` | 15.2 | ✅ | 🚫 | n/a | Once per build. |
| `buildEnd` | 15.2 | ✅ | 🚫 | n/a | Once per successful build with a summary. |
| `resolveId` | 15.2 | ✅ | 🚫 | n/a | `(specifier, from) → id \| null`. |
| `load` | 15.2 | ✅ | 🚫 | n/a | `(id) → code \| null`; virtual modules work. |
| `transform` | 15.2 | ✅ | ✅ | n/a | `(code, id) → code`; also reaches `compile_module`. |
| `renderChunk` | 15.2 | ✅ | 🚫 | n/a | `(file, code) → code`. |
| `generateBundle` | 15.2 | ✅ | 🚫 | n/a | Called with the final bundle. |
| `transformHtml` | 15.2 | ✅ | 🚫 | n/a | `(route, html) → html` in the prerender finalizer. |
| `handleHotUpdate` | 15.2 | ✅ | 🚫 | n/a | `({path}) → {reload: bool}` in dev. |
| `routes` | 15.2 | ✅ | 🚫 | n/a | Contributes route entries at scan time. |
| `typegen` | 15.2 | ✅ | 🚫 | n/a | Contributes `.d.ts` fragments after a build. |
| `diagnostics` | 15.2 | ✅ | 🚫 | n/a | Adds project diagnostics to `oxide check`. |
| `serverMiddleware` | 15.2 | ✅ | 🚫 | n/a | Contributes middleware source; alias `middleware`. |
| `configureServer` | 15.2 | ✅ | 🚫 | n/a | Called with the dev server handle. |
| `adapter` | 15.2 | ✅ | 🚫 | n/a | May select the deploy target; last result wins. |
| `renderHead` | 15.2 | ✅ | 🚫 | n/a | Merges meta/link/style/script into the render head. |
| `cacheHandler` | 15.2 | ✅ | 🚫 | n/a | Selects the cache backend before the stack is built. |

## Argument shape

JS hooks are positional on the plugin side, as in Vite. The Rust bridge sends
a named object, and the worker maps it through a documented table:

| Hook | Arguments |
|---|---|
| `transform` | `code, id` |
| `resolveId` | `specifier, from` |
| `load` | `id` |
| `renderChunk` | `file, code` |
| `generateBundle` | `files` |
| `transformHtml` | `route, html` |
| `config`, `configResolved` | `config` |
| `buildStart` | — |
| `buildEnd` | `result` |
| `handleHotUpdate` | `ctx` |
| other single-argument hooks | the one payload value |

## Route and middleware pools

`routes` and `serverMiddleware` run during `scan_routes`. A lazily-created
tooling pool is passed by `build`, `dev` (boot and HMR rescan), `start`/serve,
`routes`, `analyze`, and `check`. A project that declares neither hook never
starts an extra Node child.

## What "partial" means concretely

Only WASM `transform` is wired today; the manifest accepts `load` and
`resolveId` names, but the CLI does not invoke them yet. Native plugins are
feature backends selected by config (`images`, `fonts`, `i18n`, `mdx`,
`tailwind`, `manifest`); they do not participate in the JS hook chain.

Declaring an unsupported hook at load time is a **fail-at-load** today
for WASM components (`PLG-HOOKS` naming the missing export); the JS side is
more forgiving (the plugin loads; a hook it did not export is skipped).

## Testing the compat surface

- `crates/plugin/tests/wasm_tdd.rs` — WASM hook export probing + transform
- `crates/cli/tests.rs::build_transform_hook_*` — JS-side `transform`
- `crates/cli/src/tests.rs` — route/middleware hook fixtures and adapters
- `crates/jshost/tests/integration.rs::v8_plugin_ops_*` — V8 `plugin:load` /
  `plugin:call` parity, including positional argument order

## Plugin kinds and load failure

- A `js` plugin that declares only unsupported hooks **loads fine**; the
  pipeline just never calls it.
- A `wasm` plugin that declares a hook its component does not export
  fails at load (`PLG-WASM`).
- A `native` plugin whose `name` is not a known built-in fails at config
  load (`CFG-PLUGIN`).

These rules keep a misconfigured plugin visible without breaking a build
that would have silently ignored it.
