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
