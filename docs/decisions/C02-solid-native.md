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
