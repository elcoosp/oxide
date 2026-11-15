# 2026-11-15

## C02 — Solid native compiler: the dependency arm LANDED (upstream core)

**Decision.** The re-open record below (2026-11-14) set the re-run
condition — "when upstream publishes a crates.io crate or a public Cargo
tree, re-run the dependency arm: swap the backend's `compile` body to the
crate, bump `version()`, keep the refusal shapes, re-run the parity gate".
The condition is now **met to the letter**: the `solidjs/solid` `next`
branch carries `packages/compiler` as a public Cargo tree.

**The measured facts (2026-11-15).**

1. Upstream: `solidjs/solid` `next` at `packages/compiler` is the Rust
   crate behind npm `@solidjs/compiler` 2.0.0-rc.21 — `[package] name =
   "solidjs-compiler"`, version 0.0.0, edition 2024, `crate-type =
   ["cdylib", "rlib"]`, `publish = false`. HEAD at the measurement: `rev =
   7ea2a02e9edc976e4be4d060c7e65ba85d10b42c` (a full-SHA pin, the letter
   the kill record asked for).
2. Features: `default = ["node", "tsrx"]`; `default-features = false`
   builds the pure Rust `rlib` — `node` is the NAPI host binding (off:
   no Node host in the Rust build) and `tsrx` is the TSRX frontend (off:
   it would drag a second, git-pinned oxc 0.140 fork; a `.tsrx` source
   refuses loudly onto `CMP-SOL-001`).
3. oxc: the crate pins its own oxc **0.144** stack; the workspace rides
   oxc 0.155. No skew conflict — the C01 seam is string in / string out,
   no oxc type crosses (the same shape as verter_core's 0.117 and
   oxc_react_compiler's 0.143 skews). A Cargo git dependency takes `rev`
   OR `branch` — the full-SHA `rev` IS the branch pin.
4. API: `compile(source, &CompileOptions) -> Result<CompileOutput,
   CompileError>` with `Generate::{Dom, Ssr, Universal, Dynamic}`,
   `module_name`, `hydratable`, `dev`, `source_names` (the JS surface
   defaults each kind to `dev`), `lower_show` (the JS surface defaults it
   to `!dev`), `source_map`, and `CompileOutput { code, source_map, css,
   css_hash }` / `CompileError { kind: Parse|Configuration|Transform,
   message }`. The Rust API is upstream-unstable (pre-1.0) — the exact
   rev pin is the stability answer.

**What landed.** The in-repo JSX/reactivity transform
(`src/native/solid.rs`, the 1.7k-line walk the re-open built) is
**deleted**; the backend is a thin adapter: TS strip (workspace oxc 0.155
transformer, `only_remove_type_imports` sucrase parity, JSX untouched)
BEFORE the compile — the upstream core parses `.tsx` but does not strip
annotations, and stripping first keeps its source map valid — then
`compile()` with `module_name: "@solidjs/web"`, `hydratable: true` (both
targets, the worker oracle's flags), `generate` mapped from
`CompileRequest`, the dev flags mapped (`source_names`, `lower_show`,
`source_map`), the worker's `solid-js/web` → `@solidjs/web` rewrite after
(offset-neutral: both specifiers are 12 chars, the map survives). Every
upstream failure maps onto `CMP-SOL-001` with the upstream stage +
diagnostic riding the message and the Node-pool fix. `version()` is
`2.0.0-rc.21+7ea2a02` (npm release + short rev) — the D40 bump
invalidates every artifact the in-repo transform ever emitted.
`CompileOutput` carries `source_map: Option<String>` now (the first
native backend to emit one; dev only — the production pipeline does not
consume per-file native maps yet, the pool emits none).

**The supported surface widened.** The v1 refusal list the in-repo
transform carried (spreads, SSR fragments, object `style`/`classList`,
`innerHTML`) is gone — the 2.0 core compiles those natively (`_$spread`,
SSR array templates, ...). `use:` directives now refuse (they were
REMOVED in Solid 2.0) with the upstream migration note. The spike pins
(`solid_native_spike.rs`) hold the upstream shapes (`_$template` +
`_$getNextMarker` walks, `_$scope` effects, `_$memo`-lowered `<Show>`,
SSR `<!--$-->` hydration markers, dev `sourceNames` + the V3 map) and the
byte determinism the CAS contract needs.

---

# 2026-11-14

## C02 — Solid native compiler: RE-OPENED, LANDED in-repo (WP V2-C02)

**Decision.** The kill record (`C02-solid-kill.md`, same day) pinned two
measured facts — no crates.io crate, no pin-able upstream repo — and set
re-run conditions: a Rust compiler on crates.io, or a standalone repo a
full-SHA `rev` can pin. The re-run condition is now met **in spirit**: the
official artifact exists and is real, but the letter (a sourceable Cargo
dependency) stays unmet, so the backend lands as the **in-repo transform**
over the workspace oxc stack instead of a vendored upstream.

**The measured facts (2026-11-14, re-measured).**

1. npm: `@solidjs/compiler` is real and official — dist-tags `latest`
   2.0.0-rc.2, `next` **2.0.0-rc.21**. Its README: "Solid 2.0's native
   Oxc JSX compiler … The Node `transform()` interface is the supported
   public contract; the Rust `compile` API is unstable.". The package is
   the NAPI loader (`napi build --manifest-path ./Cargo.toml`,
   `optionalDependencies` per-platform: darwin/linux/windows/wasm32),
   repository.directory `packages/compiler`.
2. crates.io: `solidjs-compiler` still does not exist (the sparse index
   answers empty; the API 404s; the alternates `solid-compiler`,
   `solid_compiler`, `solidjs` too).
3. Upstream: the `solidjs/solid` `main` clone (HEAD `e2a181d`) carries
   **no `packages/compiler` and zero `Cargo.toml`** — the Rust core
   builds from a tree that is not public yet, so no git/rev dependency is
   possible either.

**Consequence (the dependency arm).** No external dependency rides
`native-solid` — the feature gates the in-repo backend only. The
transform implements the worker oracle's contract in-repo over the
workspace oxc 0.155 stack (`crates/compiler/src/native/solid.rs`):
parse → the TypeScript pass (JSX preserved, `only_remove_type_imports`
for sucrase parity) → the JSX walk → `@solidjs/web` runtime emission
(`moduleName: "@solidjs/web"`, `generate: 'dom' | 'ssr'`,
`hydratable: true`, the `solid-js/web` → `@solidjs/web` import rewrite)
→ codegen. String in / string out across the C01 seam — no AST type
crosses, no oxc skew is introduced (the workspace stack is THE stack).

**The v1 scope** (pinned by `crates/compiler/tests/solid_native_spike.rs`).
Supported: static/dynamic attributes (string `style`/`class`),
delegated (`onClick` → `$$click` + the module-tail `delegateEvents`) and
non-delegated (`on:click`) events, dynamic text (DOM `insert` with `<!>`
markers; SSR `escape`), nested dynamic elements (named
`firstChild`/`nextSibling` walks), components with getter props,
fragments in DOM output (arrays), TSX and JSX. Refused loudly with
`CMP-SOL-001` (the fix names the Node pool): spreads, `ref`, `use:` and
`oncapture:` directives, object-literal `style`/`classList` (string
forms are supported), `innerHTML`, fragments in SSR output, unparseable
sources. Every refusal compiles on the Node tooling pool — `auto` falls
back, `native` surfaces the refusal.

**Deltas vs the spec's C02 sketch (recorded, not silent).**

1. The vendoring arm (`vendor/solidjs-compiler`, `UPSTREAM_REV`) is void
   per the brief's §0a — there is nothing to vendor at upstream `main`;
   the in-repo transform replaces it and `version()` is this transform's
   own revision (`1.0.0`), the D40 CAS dimension to bump on any
   output-shape change.
2. The spike file is `solid_native_spike.rs` (the C03/C04 sibling
   spelling), not the spec's `solid_native.rs`; it is NOT `#[ignore]`d —
   the transform is in-repo, so the shape pins run under the feature
   with no Node host needed. The Node-oracle equivalence stays with
   `scripts/compiler-equiv.sh solid` (the capable-host acceptance).
3. The `hydratable: true` flag the worker hardcodes is reflected in the
   SSR shape (`ssrHydrationKey()` after every tag name); the DOM shape
   is hydration-agnostic. Full babel byte-fidelity is not claimed — the
   emitted shape is runtime-correct for the pinned subset, and the
   rendered-HTML equivalence is the harness gate.

**Re-run conditions (updated).** When upstream publishes a crates.io
crate or a public Cargo tree, re-run the dependency arm against
`@solidjs/compiler`'s Rust core: swap the backend's `compile` body to
the crate (the seam stays string-in/string-out), bump `version()` (D40
invalidates), keep the refusal shapes for the constructs the crate
itself refuses, and re-run `scripts/compiler-equiv.sh solid` plus the
spike pins as the parity gate.
