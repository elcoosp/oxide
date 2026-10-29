---
title: Continuous integration
description: Twenty-two jobs across three operating systems, what each one asserts, and how to run the same gate locally.
order: 8
---

# Continuous integration

`.github/workflows/ci.yml` runs **22 jobs** on every push to `main` and every
pull request. Twenty-one run per push; the twenty-second (Miri) runs nightly,
because it needs a nightly toolchain and is minutes-slow.

## The matrix

Three job names fan out across the platform matrix — `rust`, `conformance` and
`playwright` each run on `ubuntu-latest`, `macos-latest` and `windows-latest`,
which accounts for 9 of the 22. The remaining thirteen are single-platform:
`node`, `wasm32-edge`, `plugin-fixtures`, `fixtures-v8`, `bench-gate`, `redis`,
`fuzz-decode`, `stale-strings`, `launch-content`, `spec-anchors`,
`hmr-latency`, `jshost-e2e`, `doc-rot`.

## What a failure means

| Jobs | Assertion |
|---|---|
| `rust ×3` | fmt, clippy at `-D warnings`, then the full workspace test suite under nextest |
| `conformance ×3` | every adapter passes the same assertion set — a drift blocks the release |
| `playwright ×3` | the browser matrix serves a real build and hydrates |
| `node` | every TypeScript package builds and its own tests pass |
| `wasm32-edge` | `oxide-fw-core-wasm` compiles for `wasm32-unknown-unknown` |
| `fixtures-v8` | the V8 backend fixture gate on macOS |
| `bench-gate` | deterministic byte counts must not move more than 5%; wall-clock is information only |
| `redis` | the `#[ignore]`d integration test against a live Redis |
| `plugin-fixtures` | the WebAssembly plugin ABI still round-trips |
| `spec-anchors`, `stale-strings`, `launch-content`, `doc-rot` | the spec, the copy and the docs still agree with the code |

## The action-setup version pin

Every `pnpm/action-setup` step pins `v6.1.0` explicitly, not `@v6`. This is
not paranoia: `pnpm/action-setup` v6.0.x bootstraps pnpm 11 via npm and then
runs `pnpm self-update <target>`. pnpm 12 ships a **native executable**, so
the shim v6.0.x writes points at an extensionless file and every job dies at
the `pnpm store path` step. v6.1.0 adds a native pnpm-12 bootstrap. Pinning
the patch rather than the major means a future v6.0.x re-tag cannot silently
bring the broken path back.

## Running the same gate locally

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --no-fail-fast

pnpm -r --filter '!./packages/oxide-website' --filter '!./examples/*' run build
for p in core react vue svelte solid; do pnpm --filter "@oxide-fw/$p" test; done
pnpm -C conformance test

./scripts/check-spec-anchors.sh
./scripts/check-stale-strings.sh
./scripts/check-launch-content.sh
```

`cargo test --workspace` also works, but nextest is what CI runs and its
per-test timeout (`.config/nextest.toml`, 90 seconds) is what bounds a hang to
a diagnosable failure instead of a job at 0% CPU for an hour.
