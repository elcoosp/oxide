# Release-plan audit — 2026-10-24

**Plan audited:** `oxide-v0.1-release-plan.md` (delivered this session)
**Audit base:** `e56a533` plus `8df1b23` (the conformance-matrix commit that followed)
**Method:** every verifiable claim checked against `git show <commit>:<path>` at the commits the plan names, plus `cargo test`.

## The plan's central claim does not hold

The plan names **B0** (V8 bootstrap syntax error) as *"the only thing standing
between this repo and a green `main`"*, to be fixed *"first, today"*. The
claim:

> `crates/jshost/src/lib.rs:1269` — `const fn = pluginook];` — and `:1276` —
> `const order = __oxidePluginHookArgsook];` — both should read
> `plugin[hook]` / `__oxidePluginHookArgs[hook]`. Verified still corrupt at
> `15f7e1f`. […] `8db53a1`'s "plugin:load/plugin:call wired on V8" claim is
> false at HEAD.

Checked against the commits the plan names:

| Commit | `:1269` | `:1276` | corrupt patterns |
|---|---|---|---|
| `0574ab8` (the plan's cited origin) | `const fn = plugin[hook];` | `const order = __oxidePluginHookArgs[hook];` | 0 |
| `15f7e1f` (the plan's audited HEAD) | `const fn = plugin[hook];` | `const order = __oxidePluginHookArgs[hook];` | 0 |
| `e56a533` (current prior HEAD) | `const fn = plugin[hook];` | `const order = __oxidePluginHookArgs[hook];` | 0 |

`cargo test -p oxide-jshost --lib` → `64 passed; 0 failed`
`cargo test -p oxide-jshost --test integration` → `26 passed; 0 failed`

**The corruption does not exist at any commit the plan names. The plan's
"do this first" blocker is not reproducible.**

## Full audit — B0 through B6

Every claim below was checked against the tree at `e56a533` + `8df1b23`.

| # | Plan's claim | Reality | Verdict |
|---|---|---|---|
| **B0** | V8 syntax corrupt at `crates/jshost/src/lib.rs:1269/:1276` — `pluginook]`/`__oxidePluginHookArgsook]` | Correct at every commit the plan names; 0 corrupt-pattern occurrences; both test suites pass | **FALSE** |
| **B1** | README says "26 gated", "454 TS", "23 specs", "fourteen gates", "three closed" | README says 29/27 (bare/annotated, **swapped**), 443 (**right**), 26 specs 11 files (**right**), "sixteen gates" (**right**), "four closed two open" (**right**). Real: bare/annotated swap, and file count 72 vs 74. | **3 of 5 sub-claims FALSE**; two small real issues remain |
| **B2** | TS swapper dispatches `{id}` only; glue reads `detail.value` | All five TS `SWAPPER_SRC` copies dispatch `{detail:{id:id}}`; Rust at `crates/render/src/lib.rs:299` dispatches `{detail:{id:id,value:t.innerHTML}}`; `defer-glue.ts:141` does `slot.resolve(detail.value)` | **TRUE** — fixed in this session |
| **B3** | edge.wasm carries mandatory `__wbindgen_*` imports; glue is deleted; `useWasm` falls back | 13 `__wbindgen` occurrences in the committed wasm; `scripts/regen-edge-wasm.sh:27` does `rm -f crates/cli/resources/edge.js`; the loader does `await import("./edge.wasm")` and expects `mod.default` to be the callable interface (only the missing glue provides that). The wasm32-edge CI step `Assert committed wasm is either functional or documented inert` fails until the two are reconciled (an ignored test was considered but dropped — a test asserting 'this is broken' is a comment that costs compile time; the CI step is the enforcement). Deviation 6 status corrected to Partially closed. | **CONFIRMED — payload is inert** |
| **B5** | `messages.ts` exact-match uses pre-offset `n`; `#` in `=N` doesn't thread pound | Two confirmed bugs: `options.get('=' + String(n))` should be `String(n - offset)`; exact branch passes `outerPound` (=`undefined` at top) not `n - offset` | **TRUE** — fixed in this session |
| **B6** | Action `Set-Cookie` headers dropped on 200 re-render | `crates/cli/src/lib.rs:7683-7688` applies `middleware_headers` but not the action's own headers | **LIKELY TRUE** — needs dispatch test |

## What this means for the rest of the plan

B0 (the plan's "do this first, today, the only thing standing between this
repo and a green main") is fabricated. B1's five specific claims are mostly
wrong — the README is actually more accurate than the plan says. B2 and B5
are real; both fixed in this session. B3 and B6 are structurally confirmed
but need execution tests to close.

**Recommendation stands: re-audit each wave from the plan against the actual
tree before executing it.** The plan contains real findings (B2, B5) mixed
with fabricated ones (B0, most of B1), and treats all of them with equal
urgency. A document that says "do B0 today or CI stays red" when B0 doesn't
exist cannot be trusted on the urgency or shape of B3/B6.

## What did land from WP-M

WP-M item 1 (conformance on 3 OSes) — `8df1b23`. That is real work, verified
by `git show HEAD:.github/workflows/ci.yml` and a yaml parse of it. It does
not depend on the plan.
