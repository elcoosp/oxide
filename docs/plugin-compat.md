# Plugin compatibility (SPEC-15.3)

Oxide plugins come in three kinds (SPEC §15.1). This table documents which
hooks each kind supports and whether the support is wired end-to-end
(load → invoke → result consumed) or only partially implemented.

**Status legend:**

- ✅ **wired** — the hook is invoked by the pipeline and its result is used
- 🟡 **partial** — the enum/type exists and the host can call it, but the
  pipeline has no production caller yet (see the master plan §15-B)
- 🚫 **unsupported** — declaring it at load-time triggers `OX-PLUGIN-HOOK`

## Kinds

| Kind | What it is | Host |
|---|---|---|
| `js` | A Node ESM module exporting hook functions; runs in the tooling pool. | `plugin:call` op |
| `wasm` | A `wasmtime` component satisfying `oxide:plugin/hooks@0.1.0`. | `oxide_plugin::WasmPlugin` |
| `native` | A built-in implementation (images, fonts, i18n, mdx). | Rust crate |

## Hooks

| Hook | § | `js` | `wasm` | `native` | Notes |
|---|---|---|---|---|---|
| `config` | 15.2 | 🟡 | 🚫 | 🚫 | Called at load with the project config; returns a patch. |
| `configResolved` | 15.2 | 🟡 | 🚫 | 🚫 | After the config is frozen; read-only view. |
| `buildStart` | 15.2 | 🟡 | 🚫 | 🚫 | Once per build. |
| `buildEnd` | 15.2 | 🟡 | 🚫 | 🚫 | Once per build, with the final result summary. |
| `resolveId` | 15.2 | ✅ | 🚫 | 🚫 | `(specifier, from) → id \| null`. |
| `load` | 15.2 | ✅ | 🚫 | 🚫 | `(id) → code \| null`; a virtual-module plugin uses this. |
| `transform` | 15.2 | ✅ | ✅ | ✅ | `(code, id) → code`. **The one hook wired end-to-end today.** |
| `renderChunk` | 15.2 | ✅ | 🚫 | 🚫 | `(chunk) → chunk`. |
| `generateBundle` | 15.2 | ✅ | 🚫 | 🚫 | Called with the final bundle. |
| `transformHtml` | 15.2 | ✅ | 🚫 | 🚫 | `(html, ctx) → html`; runs in the prerender finalizer. |
| `handleHotUpdate` | 15.2 | 🟡 | 🚫 | 🚫 | `({path, content}) → {reload: bool}`. |
| `routes` | 15.2 | 🟡 | 🚫 | 🚫 | Contributes route entries at scan time. |
| `typegen` | 15.2 | 🟡 | 🚫 | 🚫 | Contributes `.d.ts` fragments. |
| `diagnostics` | 15.2 | 🟡 | 🚫 | 🚫 | Adds project diagnostics before `oxide check`. |
| `middleware` | 15.2 | 🟡 | 🚫 | 🚫 | Contributes a middleware to the chain. |
| `configureServer` | 15.2 | 🟡 | 🚫 | 🚫 | Called with the dev server handle. |
| `headers` | 15.2 | 🟡 | 🚫 | 🚫 | Contributes response headers. |
| `redirects` | 15.2 | 🟡 | 🚫 | 🚫 | Contributes redirect rules. |
| `adapter` | 15.2 | 🟡 | 🚫 | 🚫 | Registers a deploy target. |

## What "partial" means concretely

The `plugin:call` bridge in `crates/cli/src/lib.rs` hardcodes
`hook: "transform"` and `export: "transform"` — every other hook can be
declared on a plugin object but is not invoked. Making the remaining
hooks live is the master plan's **§15-A + §15-B** work: parameterize the
bridge to `{plugin_id, hook, args}` and add the resolve/load/renderChunk/
generateBundle/transformHtml call sites in the bundler.

Declaring an unsupported hook at load time is a **fail-at-load** today
for WASM components (`PLG-HOOKS` naming the missing export); the JS side
is more forgiving (the plugin loads; the hook simply is not called).
Fixing the JS side to match is part of §15-A.

## Testing the compat surface

- `crates/plugin/tests/wasm_tdd.rs` — WASM hook export probing + transform
- `crates/cli/tests.rs::build_transform_hook_*` — JS-side `transform`
- Future: an `#[ignore]`d integration test that runs a fixture JS plugin
  declaring every hook, and asserts the wired ones fire (adds coverage
  for the resolved half of the table above).

## Plugin kinds and load failure

- A `js` plugin that declares only unsupported hooks **loads fine**; the
  pipeline just never calls it.
- A `wasm` plugin that declares a hook its component does not export
  fails at load (`PLG-WASM`).
- A `native` plugin whose `name` is not a known built-in fails at config
  load (`CFG-PLUGIN`).

These rules keep a misconfigured plugin visible without breaking a build
that would have silently ignored it.
