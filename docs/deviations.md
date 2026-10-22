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
