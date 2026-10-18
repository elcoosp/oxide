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
