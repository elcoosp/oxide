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

## What this means for the rest of the plan

The plan's other claims (B1 README staleness, B2 swapper `detail.value`,
B3 edge-wasm inert, B5 ICU exact-match, B6 action headers, R1–R5 release
criteria) have **not** been audited in this session. Given that B0 — the
plan's most-verified-sounding claim, with a specific file, specific lines,
and a specific commit — is factually wrong, the remaining claims should be
treated as unverified until each is checked the same way.

**Recommendation: re-audit each wave from the plan against the actual tree
before executing it.** Do not spend author-days on B1–B8 on the strength of
a document whose one hard, falsifiable claim was wrong.

## What did land from WP-M

WP-M item 1 (conformance on 3 OSes) — `8df1b23`. That is real work, verified
by `git show HEAD:.github/workflows/ci.yml` and a yaml parse of it. It does
not depend on the plan.
