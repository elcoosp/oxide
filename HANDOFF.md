# Oxide handoff — Session 2026-10-19 (Wave 3 + partial Wave 4)

**Date:** 2026-10-19
**HEAD:** `f5a9208` (see `git log --oneline -10` for the full session)
**Gates at handoff:** `cargo test --workspace` green · all node packages green ·
`conformance/run.ts` green (24×4).

This document replaces the previous "Wave 1 + Wave 2 complete" handoff.
That session ended at `7e0c659`; this one began there, fixed a stale
compile break, then executed the **complete Wave 3 backlog** and started
Wave 4 hygiene. All items below have load-bearing tests.

---

## Session summary — commits (oldest → newest)

| Commit | Wave | What |
| --- | --- | --- |
| `05da397` | fix | `inject_before_close_tdd` initializer missing `OxideRequest.trace` |
| `61a71f4` | chore | Point `Cargo.toml` repository at `github.com/elcoosp/oxide` |
| `c6000a5` | W3-1 | `RouteEntry.needs_parent` field (scanner heuristic + archive + 21 literals) |
| `035aba2` | W3-1 | Last-layout + page load run in parallel when `needs_parent = false` |
| `2c1782d` | W3-1 | Peak-concurrency probe test (load-bearing: `max_active == 2` / `== 1`) |
| `10458e2` | W3-2 | Prerender error-status carried through L0 seed manifest |
| `6eae760` | W4.7 | Correct stale middleware + `v8_ops` comments; dedupe WORKLOG |
| `88c9aa2` | W3-7 | Middleware chain runs outer→inner and threads `locals` |
| `2e2983f` | W3-8 | `searchParams` exposed on `load(event)` / `action(event)` at runtime |
| `bd7adfe` | W3-4 | Redis + S3 L2 cache handlers (feature-gated) |
| `32a51d6` | W3-4 | `build_framework_stack` attaches real drivers (worker path) |
| `c86cafb` | W3-5 | `build_transform_hook` returns `None` without a wasm plugin (negative arm) |
| `665dba5` | chore | Enable `wasmtime/wat` for future wasm fixtures |
| `29f5485` | W3-6 | V8 `compile_module` op wired through `transform_handler` |
| `f5a9208` | W3-3 | Playwright specs: middleware / loading / socket / SSE |

---

## Wave 3 — status

| # | Item | Status | Test |
| --- | --- | --- | --- |
| 1 | Parallel layout loads (§9.2) | ✅ done | `layout_and_page_overlap_when_page_does_not_call_parent` + sequential twin |
| 2 | Prerender error-status sidecar (§10.2 step 5) | ✅ done | `write_prerender_carries_500_status_for_marker_body` + 200 twin |
| 3 | Playwright specs for 4 smoke fixtures | ✅ specs authored; runs on demand | `tests/playwright/tests/{middleware,loading,socket,sse}.spec.ts` |
| 4 | Redis / S3 cache handlers | ✅ done | `s3_handler_roundtrips_with_in_memory_store` + `redis_roundtrip_and_tag_invalidation` (skips w/o env) + `remote_cache_handler_maps_handler_config` |
| 5 | WASM plugin e2e fixture | 🟡 negative-arm only | `build_transform_hook_returns_none_when_no_wasm_plugin_configured` |
| 6 | V8Host `compile_module` | ✅ done | `op_oxide_compile_module_roundtrips` |
| 7 | Middleware chain (§8.5) | ✅ done | `middleware_chain_runs_outer_to_inner_and_threads_locals` |
| 8 | typegen `searchParams` (runtime) | ✅ done (typegen was already done) | `loader_event_includes_parsed_search_params` |

**Item 5 — the positive-arm test.** The negative arm is committed and
load-bearing. The positive arm (a real string-ABI component firing the
transform) needs a pre-compiled `.wasm` fixture. Hand-authored WAT that
implements the canonical ABI string return has proven fragile — the
current `MUTATE_WAT` gets rejected by `wasmtime/wat` with an opaque
syntax error. The plan: check in a pre-built `.wasm` binary (compiled once
with `cargo component build` from a Rust crate under `crates/plugin/fixtures/`)
and load it in the test. Not attempted this session (bounded scope).

---

## Wave 4 — status

| # | Item | Status |
| --- | --- | --- |
| 4.2 | `.github/workflows/ci.yml` `push.branches` | ✅ already `[main]` (the "broken `ain]`" claim was stale) |
| 4.3 | Root `Cargo.toml` repository URL | ✅ fixed in `61a71f4` |
| 4.6 | `WORKLOG.md` dedupe (SPEC-09, SPEC-17) | ✅ done in `6eae760` |
| 4.7 | Stale comments (`v8_ops.rs`, `cli/lib.rs` middleware) | ✅ done in `6eae760` |
| 4.1 | `benchmarks/report.md` | ⏳ not re-measured (needs `benchmarks/run.sh` + a quiet host) |
| 4.4 | `HANDOFF.md` | ✅ this document |
| 4.5 | `docs/architecture.md` regenerate | ⏳ not done (large, 1100+ commits stale) |
| 4.8 | Commit-message policy | ✅ being observed this session |

**Wave 4.5 (`docs/architecture.md`)** is the one remaining mechanical
item. It needs the whole workspace walked and prose re-written from
current `crates/` layout — a dedicated session's worth of work, not a
drive-by. The stale file is at `docs/architecture.md`; the plan is to
regenerate from `cargo metadata` + a manual pass over each `crates/*/src/lib.rs`
module docstring.

---

## How to run the full gate set

    cargo test --workspace --no-fail-fast
    for p in oxide react vue svelte solid; do (cd packages/$p && node --test test/*.test.ts); done
    (cd conformance && node run.ts)
    cargo build -p oxide-cli
    pnpm --filter @oxide/playwright install-browsers
    pnpm --filter @oxide/playwright test

`README.md` is currently dirty from a user edit (badge counts) — not
staged by this session's commits; commit or discard as you see fit.

---

## Gotchas learned this session

1. **`#[op2]` changes the function's call signature** — you cannot invoke
   a decorated op as a normal async fn. Extract the body into a plain
   `fn_impl` and have both the op and the test call it.
2. **`wasmtime/wat` feature must be enabled** for `Component::new` to
   accept WAT text; without it, "extra tokens remaining after parse".
   Even with it, hand-authored canonical-ABI string WAT is fragile.
3. **`redis` needs `connection-manager` feature** for `ConnectionManager`
   — `tokio-comp` alone is not enough.
4. **`object_store` v0.11 `PutPayload`** wraps `bytes::Bytes`, not a
   plain `Vec<u8>`.
5. **Deno's `#[op2]` `#[string]`** on an async fn returning `Result<T, JsErrorBox>`
   is the shape `op_oxide_fetch` uses — the `compile_module` op should
   match it if it needs a fallible return.
6. **V8 prelude is a string literal in `lib.rs`** — a JS syntax error in
   it fails at runtime, not compile time. Test via the prelude test path
   (`v8_prelude_tdd` with `--ignored`).

---

## Where to start next

1. `git status` — confirm clean (except `README.md`).
2. Finish Wave 4.5 (`docs/architecture.md`) — the largest remaining item.
3. Then Wave 4.1 (`benchmarks/report.md`) — needs a quiet host.
4. Optional: item 5 positive arm (WASM fixture) — see §"Item 5" above.

If a claim here is contradicted by the tree, **trust the tree** and update
this file.
