# Deviations from the v0.1 spec

## Per-chunk incremental bundling (5-A)

**What the spec asks.** "Edit one component, one transform + one chunk
re-hash."

**What lands.** An environment-level skip: a per-env fingerprint (excluding
`*.server.*` for the client env) decides whether to re-bundle at all, so a
server-only edit leaves the client bundle valid. Per-route *prerender* reuse
is separate and complete. Per-route chunk hashes are recorded in
`.oxide/graph.json` (`route_chunks`), so "did this route's chunks change?" is
measurable.

**Why not per-chunk.** Rolldown 1.2.13 exposes no incremental/cache API and
no per-chunk write control. The bundle is one `write()` call; there is no
hook between "this module changed" and "re-emit this chunk". A per-chunk
build needs the bundler to publish its module graph and accept a
cache/diff - that is a bundler change, not a CLI change.

**What would change it.** A Rolldown version exposing an incremental cache,
or a switch to a bundler that does. The `route_chunks` records are the diff
target for that work.

## WOFF2 font output (16-B)

**What the spec asks.** WOFF2.

**What lands.** TrueType subset output, CSS `format('truetype')`.

**Why.** No published crate provides a WOFF2 encoder (`woff2` / `woff2-sys`
are not on crates.io; `subsetter` emits TrueType). Hand-writing one means the
WOFF2 glyf/loca transform plus Brotli - a project in itself.

**What would change it.** A published encoder crate.
`crates/assets/tests/font_woff2_deviation_tdd.rs` is the tripwire: it asserts
the `.ttf` extension and the documented deviation, so an encoder swap updates
both deliberately.

## Dev route additions require a dev-server restart (WP-C1)

**What the spec asks.** Adding a route file under `routes.dir` should render in
dev without editing `oxide.config.ts` or restarting (§13 hot updates).

**What lands.** Dev resolves components from the route table, so any route that
exists at boot renders without config edits, and edits to existing components
are picked up by the watcher (module-generation bump). A **newly created or
deleted route file** is not re-scanned: `cmd_dev` discovers routes once at boot
and the HTTP dispatch table is built from that tree.

**Why.** Reloading the route table means rebuilding the matcher, the server
manifest view, and the dev component map, then swapping them atomically under
live connections. That is a separate change to `oxide-dev`'s watcher contract;
the component-resolution half of WP-C1 (the part that unblocked config
decoupling) is done.

**What would change it.** A route-table generation in `cmd_dev` that re-runs
`scan_routes` on `Created`/`Removed` events for `routes.dir` and rebuilds
`dev_component_sources` for the new canonical ids.
## Edge `mergeHead` is JS-only (P0-1)

**What the spec asks.** The edge handler (D18) prefers the wasm export for
each of the three named functions — `edge_cache_key`, `merge_head`,
`encode_odf` — and falls back to a semantically identical JS path when
wasm is absent.

**What lands.** `edge_cache_key` and `encode_odf` prefer the wasm export
as specified. `merge_head` is JS-only.

**Why.** The Rust `merge_head` merges flat `Vec<(String, String)>` pairs
(one key per entry); the edge head model is structured
(`{title, meta: [[k,v], ...], links, json_ld}`) with per-kind dedup. They
are different algorithms, not two encodings of one. The previous wasm
path *silently* returned `[]`: `serde_json::from_str::<Vec<(String,String)>>`
fails on object input, `unwrap_or_default()` discards the error, the
returned `"[]"` parses as `[]`, and the JS fallback therefore never ran —
an empty head on every edge response.

**What would change it.** A Rust-side port of the structured head merge,
with its own tests against the edge shape, exposed as a new export
(`merge_head_structured`). Reusing `merge_head` is a type mismatch by
design, not by accident.


## Edge bundle: no externals; chunked output (P0-1b)

**What the spec asks.** §18 / D18: an `oxide build` for a Cloudflare
target should produce a deployable Worker that includes the project's
edge entry (if any).

**What lands.** `cmd_build` for `Target::Cloudflare | Target::CloudflarePages`
detects `src/edge.{ts,mjs,js,tsx,jsx}` (typed extension preferred) and
bundles it with a single-entry `BundleEnv::Edge` pass into
`dist/deploy/edge_bundle.js`. The existing `edge_handler.js` does
`import("./edge_bundle.js")` and calls its default export. A project with
no edge entry is fine — the handler's try/catch on `Cannot find module`
falls through to the built-in JS path.

**Two design decisions worth recording.**

- **No externals.** The edge handler must be self-contained: a bare npm
  import either resolves to the package's edge entry at build time or
  fails with `BLD-RESOLVE`. `node:*` imports are not special-cased —
  a project using them gets the build-time resolve error, which is the
  honest signal (Cloudflare Workers' node-compat is a runtime flag the
  build cannot know about).
- **Chunked output allowed.** Rolldown may emit shared chunks alongside
  the entry. All chunks are written to `dist/deploy/` (Cloudflare Workers
  deploys the directory as a whole); only the entry chunk is renamed to
  `edge_bundle.js`, and its relative imports of the sibling chunks
  resolve at their hashed names.

**What would change it.** A declared externalisation API
(`build.edgeExternals`), or a Rolldown option to force a single-chunk
bundle at the cost of duplication. Either is a follow-up; the current
build is deployable.

## Edge wasm build requires self-hosting layout (P0-1c)

**What the spec asks.** §18 / D18: an `oxide build` for a Cloudflare
target produces an accelerated `edge.wasm` alongside the JS handler.

**What lands.** The CI job `wasm32-edge` installs `wasm32-unknown-unknown`
and builds `oxide-core-wasm` for it. A `#[ignore]`'d test in
`crates/cli/src/tests.rs`
(`compile_edge_wasm_from_workspace_root_emits_edge_wasm`) runs the same
build on demand and asserts `dist/edge.wasm` exists and is non-empty.

**Two bugs the gate caught on first run.** Before the gate existed,
`oxide-core-wasm` did *not* actually compile for `wasm32-unknown-unknown`:

1. The `#[cfg(target_arch = "wasm32")] bindings` module was never compiled
   on host, so two type errors survived every prior gate:
   - `encode_odf` passed a `serde_json::Value` into `crate::encode_odf(&Value)`,
     which expects `oxide_odf::Value` (E0308: distinct types).
   - `decode_odf` called `serde_json::to_vec` on `oxide_odf::Value`, which
     does not implement `Serialize` by design (E0277 — ODF is the wire
     format, JSON is a lossy projection).
   Fixed by routing through the existing `Value::from_json` /
   `Value::to_json` bridges.

2. `.cargo/config.toml` carried `[build] rustflags = ["-C",
   "link-arg=-Wl,-w"]` to suppress macOS `ld64` warnings about
   v8 template-weak symbols. `-Wl,` is a compiler-driver prefix; when
   `--target wasm32-unknown-unknown` is passed, rustc invokes `rust-lld`
   directly and `rust-lld` receives the literal string `-Wl,-w` (unknown
   argument). Scoped the flag to the explicit host triple.

**Why it does not work for a real user project.** `compile_edge_wasm(root,
dist)` runs `cargo build -p oxide-core-wasm ...` with `current_dir(root)`.
A real user's oxide project is a JS/TS tree, not a cargo workspace that
contains `oxide-core-wasm` — the command exits non-zero (`no such
package`), `cmd_build` logs `edge wasm build failed` and the Worker ships
the pure-JS fallback (which is semantically identical, spec D18). The
fallback's `cacheKey` used to hash a non-canonical composite; it now embeds
a BLAKE3 implementation and emits the same `L0` key as the wasm/native path
(checked against Rust vectors), so the fallback differs only in speed. The
accelerated path only fires for the self-hosting case (root = oxide
workspace).

**What would change it.** Either (a) embed the wasm via `include_bytes!`
in oxide-cli with a `build.rs` that produces it once at CLI build time,
or (b) ship a pre-built `edge.wasm` in the oxide-cli binary's resource
set and copy it into `dist/edge.wasm` per build. Both are M-effort; the
CI gate landed here is the honest S-effort scope — the wasm *builds*;
the user-project plumbing is a separate change.

## `<Defer>` for `suspense: 'core'` adapters is designed, not shipped (§7.1)

**What the spec asks.** §7.1: "Adapters with `suspense: 'native'` use their
own mechanism; adapters with `suspense: 'core'` get boundaries from a
`<Defer>` component the core exports per framework (generated from the
adapter)."

**What lands.** The wire protocol exists and is tested — `template[data-ox-b]`
placeholder, `template[data-ox-c]` + swapper resolution, `ox:swap` dispatch,
error-boundary mapping (`conformance/suite.ts`, four assertions per adapter
against the IR renderer). The IR renderer is its reference implementation.
Vue and Svelte have **no `<Defer>`**; a `deferred(...)` value in a
`suspense: 'core'` real adapter renders its resolved value inline (the
`defer()` fixes below) but never streams.

**Why this is not a small component.** Three obstacles, each found while
attempting the implementation:

1. **Placeholder markup cannot go through VNode rendering.** A Vue
   `<Defer>` that returns `h('template', …)` renders as
   `<template data-ox-b="0"><!--[-->fallback<!--]--></template>` — Vue's
   hydration comment markers land inside the template, and the swapper
   moves `t.innerHTML` into the page, so the markers end up in the DOM.
   The placeholder must be emitted as raw HTML, which needs a render
   escape hatch (a `v-html` wrapper, or an adapter that post-processes a
   sentinel).

2. **Svelte needs a compiled component.** The package ships `.ts` only
   (`src/Image.svelte` is the one exception, declared in `exports`).
   `<Defer>` would have to be a `.svelte` file with its own build step, or
   a string-template rendered through `svelte/server` — a different
   mechanism from Vue's, which means the "one `<Defer>` per framework" in
   the spec is genuinely two implementations, not one shared one.

3. ~~**The swapper `<script>` has no nonce.**~~ **FIXED** — the nonce now
   travels `PageRenderInput.nonce` → `FrameworkRenderInput.nonce` →
   `RenderInput.nonce`, and each IR renderer passes it to
   `boundaryContent`. Both call sites populate it: the live path with
   `nonce_for(request_id)`, the prerender path with the build-time nonce it
   already stamps on its own injected tags. Verified end-to-end — the
   emitted swapper carries `nonce='…'`, and it is omitted when the
   deployment configures no CSP. This was a pre-existing bug in the IR
   path, independent of `<Defer>`, and would have blocked any streaming
   adapter.

**What was tried and reverted.** A render-scoped registry
(`createBoundaryRegistry` / `withBoundaryRegistry` / `registerBoundary` /
`deferredIdOf`) plus a Vue `<Defer>` emitting `h('template', …)`. Reverted
on hitting obstacle 1 — the emitted markup was wrong in a way that would
have shipped broken boundaries.

**What would change it.** Solve obstacle 1 first (a raw-HTML escape hatch
the adapter can use), then obstacle 2 (Svelte needs a compiled component,
so its `<Defer>` is a separate implementation). Obstacle 3 is fixed; the
nonce now reaches every streaming render.
