# Plugin compatibility (SPEC-15.3)

Oxide plugins come in three kinds (SPEC §15.1). This table documents which
hooks each kind supports and whether the support is wired end-to-end
(load → invoke → result consumed) or only partially implemented.

The hooks table below is **generated, not hand-maintained**:
`oxide_fw_plugin::compat_markdown_table()` renders it from the
kind × hook support model ([`compat`]), and
`crates/plugin/tests/compat_tdd.rs::plugin_compat_doc_embeds_the_generated_table`
pins this file to the render byte-for-byte — a support flip is a code change
plus a table re-embed, never a silent prose edit.

**Status legend:**

- ✅ **wired** — the hook is invoked by the pipeline and its result is used
- 🟡 **partial** — the host can call the hook, but a call site is still
  missing (today: the WASM `load`/`resolveId` ABI is callable through
  `WasmPlugin`'s `call_load_hook`/`call_resolve_hook`; the CLI wire that feeds
  them into the bundler's `BundleOptions.load_hook`/`resolve_hook` is pending)
- 🚫 **unsupported** — no host path. For the JS bridge — and for
  `configureServer`/`cacheHandler` on every non-native kind — declaring it
  fails at load (`PLG-HOOK`); a WASM declaration of any other hook loads but
  nothing ever invokes it
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
| `resolveId` | 15.2 | ✅ | 🟡 | n/a | `(specifier, from) → id \| null`. |
| `load` | 15.2 | ✅ | 🟡 | n/a | `(id) → code \| null`; virtual modules work. |
| `transform` | 15.2 | ✅ | ✅ | n/a | `(code, id) → code`; also reaches `compile_module`. |
| `renderChunk` | 15.2 | ✅ | 🚫 | n/a | `(file, code) → code`. |
| `generateBundle` | 15.2 | ✅ | 🚫 | n/a | Called with the final bundle. |
| `transformHtml` | 15.2 | ✅ | 🚫 | n/a | `(route, html) → html` in the prerender finalizer. |
| `handleHotUpdate` | 15.2 | ✅ | 🚫 | n/a | `({path}) → {reload: bool}` in dev. |
| `routes` | 15.2 | ✅ | 🚫 | n/a | Contributes route entries at scan time. |
| `typegen` | 15.2 | ✅ | 🚫 | n/a | Contributes `.d.ts` fragments after a build. |
| `diagnostics` | 15.2 | ✅ | 🚫 | n/a | Adds project diagnostics to `oxide check`. |
| `serverMiddleware` | 15.2 | ✅ | 🚫 | n/a | Contributes middleware source; alias `middleware`. |
| `configureServer` | 15.2 | 🚫 | 🚫 | n/a | Called with the dev server handle. |
| `adapter` | 15.2 | ✅ | 🚫 | n/a | May select the deploy target; last result wins. |
| `renderHead` | 15.2 | ✅ | 🚫 | n/a | Merges meta/link/style/script into the render head. |
| `cacheHandler` | 15.2 | 🚫 | 🚫 | n/a | Selects the cache backend before the stack is built. |

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

The WASM component ABI mirrors the same shapes with canonical names
(kebab-case — the component model refuses underscore extern names):
`transform: func(code: string) -> string`,
`load: func(specifier: string) -> option<string>` and
`resolve-id: func(specifier: string, from: string) -> option<string>`.
A `none` return declines — the next plugin, or the default, wins.

## Route and middleware pools

`routes` and `serverMiddleware` run during `scan_routes`. A lazily-created
tooling pool is passed by `build`, `dev` (boot and HMR rescan), `start`/serve,
`routes`, `analyze`, and `check`. A project that declares neither hook never
starts an extra Node child.

## What "partial" means concretely

WASM `transform` is wired end to end. WASM `load`/`resolveId` are the 🟡
rows: the host speaks their ABI (`call_load_hook`/`call_resolve_hook`, the
object-safe [`HookHost`] seam carries both), string fixtures exercise them
end to end, and the remaining half is the CLI wire that composes a loaded
component into `BundleOptions.load_hook`/`resolve_hook` alongside the JS
bridge's hooks. Native plugins are feature backends selected by config
(`images`, `fonts`, `i18n`, `mdx`, `tailwind`, `manifest`); they do not
participate in the JS hook chain.

Declaring a bridge-unsupported hook (`configureServer`, `cacheHandler`) at
load time is a **fail-at-load** today for both JS plugins and WASM manifests
(`PLG-HOOK` naming the plugin + hook + this table). A WASM component that
declares a hook its bytes do not export (or export in an uncallable shape)
**loads** — the host records the gap (`available_hooks`/`missing_hooks`) and
the call site skips the hook it cannot invoke.

## Testing the compat surface

- `crates/plugin/tests/compat_tdd.rs` — the support matrix, the generated
  table's byte-identical embed, and the WASM `load`/`resolveId`/string
  `transform` ABI end to end
- `crates/plugin/tests/wasm_tdd.rs` — WASM hook export probing + transform
- `crates/plugin/benches/compat.rs` — the matrix fold + render cost
  (`cargo bench -p oxide-fw-plugin --features bench-util -- compat`)
- `crates/plugin/examples/compat_matrix.rs` — the walkthrough
  (`cargo run -p oxide-fw-plugin --example compat_matrix`)
- `crates/cli/tests.rs::build_transform_hook_*` — JS-side `transform`
- `crates/cli/src/tests.rs` — route/middleware hook fixtures and adapters
- `crates/jshost/tests/integration.rs::v8_plugin_ops_*` — V8 `plugin:load` /
  `plugin:call` parity, including positional argument order

## Plugin kinds and load failure

- A `js` plugin that declares only unsupported hooks **loads fine**; the
  pipeline just never calls it.
- A `wasm` plugin that declares a bridge-unsupported hook fails at load
  (`PLG-HOOK`); a declared-but-unexported hook loads and rides
  `missing_hooks()` until a call site would need it.
- A `native` plugin whose `name` is not a known built-in fails at config
  load (`CFG-PLUGIN`).

These rules keep a misconfigured plugin visible without breaking a build
that would have silently ignored it.
