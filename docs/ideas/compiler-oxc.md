Researched. Here's what actually exists — and the short answer is **there is no published Rust crate for this; you must take a git dependency.**

## The dependency landscape

| Dependency | Type | On crates.io? | OXC? | Status |
|---|---|---|---|---|
| **`solidjs-compiler`** (in `solidjs/solid` `packages/compiler`) | Rust crate, `cdylib`+`rlib` | ❌ `publish = false` | ✅ oxc 0.144 | **Official**, tracks Solid 2.0 RC |
| `@solidjs/compiler` | npm wrapper around the above + platform binaries | n/a | ✅ | Official, current default in `vite-plugin-solid` |
| `@dom-expressions/compiler` | Rust → NAPI + WASI | ❌ npm only | ✅ | "Experimental AST-native JSX to DOM Expressions compiler" |
| `solid-jsx-oxc` (Frank-III) | Rust → NAPI-RS | ❌ npm only | ✅ | alpha, "aims to be a drop-in replacement for `babel-plugin-jsx-dom-expressions`" |
| `@oxc-solid-js/compiler` | Rust → NAPI | ❌ npm only | ✅ | v1.1.4, "Port of babel-jsx-dom-expressions to oxc" |
| `jsx-dom-expressions` (icewind1991) | Rust crate | ❌ git only | ❌ **SWC**, not OXC | Port of the Babel plugin to SWC |
| `oxc` + `oxc_traverse` | Rust crate | ✅ | ✅ | Generic only — JSX→`jsx()`/`createElement`, **not** Solid's `ssr()`/template output |

Verified directly: `crates.io/search?q=jsx-dom-expressions` → **0 crates**; `?q=dom-expressions` → **0 crates**.

## Recommendation

**Use `solidjs-compiler` from the Solid repo as a git dependency, with `default-features = false`.**

```toml
solidjs-compiler = { git = "https://github.com/solidjs/solid", branch = "next", package = "solidjs-compiler", default-features = false }
```

Why this one:

- It's the **authority**. It's the same crate that produces `@solidjs/compiler`, which became Solid 2.0's default compiler. Every other candidate is a third-party reimplementation chasing `babel-plugin-jsx-dom-expressions` parity.
- `crate-type = ["cdylib", "rlib"]` — the `rlib` is there precisely so it can be linked into Rust.
- `default = ["node", "tsrx"]`, with `node = ["dep:napi", "dep:napi-derive"]` — so `default-features = false` strips NAPI and TSRX, leaving a pure-Rust, in-process compiler with no Node host.
- It's not `publish = false` by accident; the npm binary is the deliverable. Git is the only channel.

## Two things to plan for

**1. OXC version skew.** `solidjs-compiler` pins `oxc_* = "0.144"`, while your workspace is on `0.153.0`. Cargo will happily build both (0.x is semver-incompatible, so they're distinct crates), and since the boundary is `&str → String` no AST types cross. But your `TransformHook`/`compile_module` signature must stay string-in/string-out — don't try to share `oxc_ast` types.

**2. It's `version = "0.0.0"` on the `next` branch** — an RC-track API. Pin a commit (`rev = "..."`) rather than tracking `branch`, or a Solid RC bump will silently change your output.

## If you'd rather not take a git dep

The only crates.io-compatible path is `oxc` + `oxc_traverse` and writing the Solid visitor yourself — that's the "native Oxc-visitor port" §6.3 names as the post-MLP optimization track. You'd be reimplementing `babel-plugin-jsx-dom-expressions`'s template-cloning + `effect()` wrapping, which is what `solidjs-compiler` already is. Realistically: vendor the git dep.

## What this replaces in your tree

Right now `precompile.rs` sends every Solid `.tsx` to `tooling:solid-compile` → `require('solid-jsx-oxc')`. Swapping to `solidjs-compiler` as an rlib lets `oxide-compiler` call it directly, collapsing the N pool round-trips + N NAPI marshalls into one in-process pass — the exact win you asked about.
Researched all four. Here's the complete picture — and the short answer is **every framework has a native-Rust path, but only three of the four are on crates.io.**

## Comparison table

| Framework | Crate | On crates.io | Type | Status | Speed vs JS |
|---|---|---|---|---|---|
| **React** | `oxc` (feature `transformer`) + `oxc_react_compiler` | ✅ v0.153.0 (matches your workspace) | Pure Rust rlib | Production, 92k+ downloads | 2–10×  |
| **Vue** | `verter_core` | ✅ 0.0.1-beta.1 | Pure Rust, no FFI  | Beta, tested on Vuetify/PrimeVue/Element Plus  | — |
| **Vue** (alt) | `vize_atelier_sfc` + `vize_atelier_dom` | ✅ | Pure Rust | Active toolchain  | 4.3–24.7×  |
| **Svelte** | `svelte-compiler` | ✅ 0.1.4 | Pure Rust on `oxc_*`  | Early (0.1.x) | — |
| **Solid** | `solidjs-compiler` | ❌ git only (`publish = false`) | Rust `cdylib` + `rlib` | Official, RC-track | ~9×  |

## Per-framework recommendation

### React — `oxc` umbrella crate (you already have it)

```toml
oxc = { version = "0.153.0", features = ["transformer", "codegen", "semantic"] }
# Optional, React Compiler's Rust port as a standalone:
oxc_react_compiler = "0.137.0"
```

This is the **only one of the four with zero caveats.** It's the exact version your workspace already pins, it does TypeScript stripping + JSX (automatic runtime) + React Refresh + the native React Compiler port in one pass, and Oxc explicitly states it *"integrates the Rust port of the React Compiler rather than the Babel-based `babel-plugin-react-compiler`"*. You're likely already calling `oxide_compiler` which uses `oxc_transformer` — so this may already be done.

### Vue — `verter_core` (JS output) or `vize_atelier_sfc` (mature toolchain)

```toml
verter_core = "0.0.1-beta.1"
```

`verter_core` is *"a pure Rust library with no FFI dependencies"* with the full pipeline: tokenizer → OXC parser → LightningCSS → Vue codegen, including `defineProps`/`defineEmits`/`defineModel`/`defineSlots`/`defineExpose`/`defineOptions` macros. API is a single `generate(input, &options, &allocator) -> CodegenResult` call.

It's beta, so if you want the more battle-tested option:

```toml
vize_atelier_sfc = "*"   # SFC orchestration
vize_atelier_dom  = "*"  # DOM compiler (or vize_atelier_vapor)
vize_atelier_core = "*"  # AST + parser
```

Vize is the toolchain Volt uses for Node-free Vue, with published benchmarks of 4.3× single-threaded and 24.7× on 32 vCPUs. `fervid` (crates.io, 0.2.0, 342 stars) is a third option but hasn't shipped in 9 months and self-describes as alpha.

### Svelte — `svelte-compiler` (the only crates.io option)

```toml
svelte-compiler = "0.1.4"
```

It exposes the full public surface: `compile`, `compile_module`, `parse`, `print`, `preprocess`, `migrate`, and routes expression parsing through OXC.

Be aware of the alternatives and why they don't work for you:

- **`rsvelte`** — byte-identical output, −25–36% build time, but **not on crates.io**; only `@rsvelte/compiler` npm + a C ABI (`crates/rsvelte_capi`, JSON in/out).
- **`svelte-rs` (MrWaip)** — 9× faster, 12,000+ passing comparisons, but **explicitly paused** ("won't be publishing new versions") and not on crates.io.
- **`rustle`** — alpha with most Svelte features unchecked. Not usable.

### Solid — `solidjs-compiler` via git

```toml
solidjs-compiler = { git = "https://github.com/solidjs/solid", branch = "next", package = "solidjs-compiler", default-features = false }
```

Same conclusion as before: it's in `packages/compiler` of the official monorepo, declared `crate-type = ["cdylib", "rlib"]`, `publish = false`, and `default-features = false` strips the `node` (NAPI) and `tsrx` features, leaving a pure-Rust in-process compiler. **Pin a `rev = "..."`** — it's `version = "0.0.0"` on an RC branch. The crate is described upstream as the *"Native Oxc JSX compiler"*.

## Proposed replacement stack

```toml
# crates/compiler/Cargo.toml (or wherever oxide-compiler lives)

# React — already there, just enable the features
oxc = { version = "0.153.0", features = ["transformer", "codegen", "semantic"] }
oxc_react_compiler = { version = "0.137.0", optional = true }

# Vue
verter_core = { version = "0.0.1-beta.1", optional = true }
# or, for the more mature toolchain:
# vize_atelier_sfc = { version = "*", optional = true }
# vize_atelier_dom = { version = "*", optional = true }

# Svelte
svelte-compiler = { version = "0.1.4", optional = true }

# Solid
solidjs-compiler = { git = "https://github.com/solidjs/solid", branch = "next", rev = "<pin>", package = "solidjs-compiler", default-features = false, optional = true }
```

## What this buys you

- **Deletes the Node pool from the build path entirely.** `precompile.rs`'s `tooling:solid-compile` / `tooling:vue-compile` / `tooling:svelte-compile` pool round-trips collapse into in-process `rlib` calls — no IPC, no NAPI marshalling, no `require()`, no per-file worker hop.
- **Collapses N round-trips into 1 pass.** `precompile_entries` currently does one `call_pool` per `.tsx`/`.vue`/`.svelte`; all four crates are synchronous `&str → Result<String>` (except Vue's `generate` which takes an allocator), so the loop becomes a plain `for`.
- **Warm builds are unaffected** (the Tier-B CAS already short-circuits), so the win is cold-build + incremental-miss, exactly where §21's "cold build 2× incumbent" and "incremental ≤ 500 ms" targets live.

## Three caveats worth planning for

1. **OXC version skew.** `verter_core`, `svelte-compiler`, and `solidjs-compiler` each pin their own `oxc_*` version. Cargo will build multiple copies (0.x is semver-incompatible). Since every boundary here is `&str → String`, no AST types cross — keep it that way.
2. **Vue and Svelte are pre-1.0.** `verter_core` is `0.0.1-beta.1`, `svelte-compiler` is `0.1.4`. Gate both behind cargo features so a regression doesn't block the other two adapters, and keep the Node pool as a fallback toggle (`framework.solid.compiler = 'node' | 'native'`) for at least one release.
3. **Solid needs a vendored fork if you want CI hermeticity.** A git dep means CI needs network access to GitHub on first fetch. If that's a problem, vendor `packages/compiler` into `vendor/solidjs-compiler/` as a path dependency — the crate has no external path deps beyond `oxc`/`serde`.
