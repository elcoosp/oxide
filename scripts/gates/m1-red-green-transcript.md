# V2-F19 gate transcripts — M1

The red→green cycle for the per-route capability permissions spike (WP
V2-F19). Recorded under `scripts/gates/` per the house TDD rule.

## RED — F19 missing at HEAD (`e64bda96`)

Before this WP, the F19 substrate was absent:

- `crates/jshost/src/perm.rs` — does NOT exist (the `PermissionSet` /
  `host_allowed` policy substrate is missing).
- `crates/config/src/lib.rs` — `OxideConfig` has no `permissions` field
  (the `PermissionsConfig` typed config + `permissions.default` is
  missing).
- `crates/router/src/typegen.rs` — `declares_permissions` does NOT exist
  (the per-route `export const permissions` export scan is missing).
- `crates/jshost/src/v8_ops.rs` — no `PERM_CTX` thread-local, no
  `PermGuard` RAII, no `check_net`/`check_env`/`check_fs` boundary
  helpers, no `op_oxide_env_get` / `op_oxide_fs_read` ops, no net check
  in `op_oxide_fetch`.
- `crates/jshost/src/lib.rs` — the worker loop installs no `PermGuard`
  at the `V8Msg::Call` site; the Node `op_dispatch` Fetch arm runs no
  advisory check.
- `docs/errors.toml` — `[PRM-001]` is `spec_only = true` (no call site
  raises it); `[CFG-PERM]` does not exist.
- `conformance/assertions/c48.ts` — does NOT exist (the R01 assertion
  for the permission denial is missing).

The hostile fixture (a `setTimeout` created OUTSIDE the request context
that calls `fetch` against a denied host) has no enforcement to close
it: under Model 1 (the thread-local seeded per call), the timer fires
after the `PermGuard` dropped, with `PERM_CTX = None` = unrestricted
under default-allow-all — the bypass. This is the RED state the spike
prototypes.

Commands (the verification at HEAD):

    $ git rev-parse HEAD
    e64bda964acc9b75ff9b5b3c686e8c6fa8d456cc
    $ test ! -f crates/jshost/src/perm.rs && echo "perm.rs MISSING"
    perm.rs MISSING
    $ grep -c "permissions" crates/config/src/lib.rs
    0   # (the field is absent)
    $ grep -c "declares_permissions" crates/router/src/typegen.rs
    0
    $ grep -c "PermGuard" crates/jshost/src/v8_ops.rs
    0

## GREEN — F19 substrate landed (this WP)

The substrate ships (all net-new), the enforcement closes the bypass
(Model 2), and the tests are green:

### V8-free substrate (runs in every `--no-default-features` build)

    $ cargo test -p oxide-fw-jshost --no-default-features --lib
    test result: ok. 80 passed; 0 failed; 0 ignored; 0 measured

    $ cargo test -p oxide-fw-config
    test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured
    (the `permissions_config` suite: 11 pins green; the pre-existing
    `auto_islands_config` failures unblocked by the `modulesConfig`
    KNOWN_KEYS fix)

    $ cargo test -p oxide-fw-router --test permissions_typegen_tdd
    test result: ok. 6 passed; 0 failed; 0 ignored

    $ cargo test -p oxide-fw-adapters --test permissions_matrix_tdd
    test result: ok. 4 passed; 0 failed; 0 ignored

### V8-gated enforcement (the `PermGuard` + the hostile fixture)

    $ cargo test -p oxide-fw-jshost --test perm_tdd
    test hostile_fixture_detached_timer_is_denied_under_model_2 ... ignored
        (boots a V8 isolate; run with --ignored — CI: jshost-e2e)
    test perm_guard_clears_on_drop ... ok
    test perm_guard_seeds_the_route_grant ... ok
    test perm_guard_seeds_unrestricted_for_a_routeless_call ... ok
    test perm_guard_seeds_unrestricted_for_an_undeclared_route ... ok
    test route_registry_returns_the_registered_grant ... ok
    test result: ok. 5 passed; 0 failed; 1 ignored

### The fmt + clippy gates

    $ cargo fmt --all --check
    (clean — zero diffs)

    $ cargo clippy -p oxide-fw-jshost --no-default-features --lib -- -D warnings
    Finished (zero warnings)

    $ cargo clippy -p oxide-fw-config -p oxide-fw-router -p oxide-fw-adapters --lib -- -D warnings
    Finished (zero warnings)

### The consumer example

    $ cargo run -p oxide-fw-jshost --example per_route_permissions
    (walks the route declaration, the PermGuard seed/clear cycle, the
    net/env/fs boundary checks, the `*.` wildcard rule, the default
    posture — exits 0)

### The bench (compiles V8-free; runs on the `bench-gate` CI job)

    $ cargo build -p oxide-fw-jshost --features bench-util --benches
    Finished (the `perm` bench compiles — `host_allowed`, `url_host`,
    the full fetch-path check; release-mode numbers ride the `bench-gate`
    CI job, ENOSPC blocks the release build in the 2-core sandbox)

## The errors-registry gate

    $ cargo test -p oxide-fw-cli --test errors_registry_tdd

The `[PRM-001]` entry flipped from `spec_only = true` to landed (the
enforcement call site raises it); `[CFG-PERM]` added (the config
validator's diagnostic). The scanner finds both covered (neither is
listed as missing). The pre-existing failures
(`registry_covers_every_production_code_literal` missing CFG-CHECK /
RFN-LOAD / SKW-POOL / SKW-SNAPSHOT; `errors_page_marks_the_spec_only_codes`
on CMP-NAT-001) are pre-existing debt at HEAD — documented in the PR
body's "remaining gaps" section, not introduced by this WP.

## Not-run (sandbox reality — 2 cores, ~4GB RAM, ~10GB disk)

The full-workspace / V8 / pnpm suites run in CI; the exact CI job names:

- `lint/rust` — `cargo clippy --workspace --all-targets --all-features -- -D warnings -W pedantic`
  (the full-workspace pedantic gate; pre-existing debt in untouched
  crates — assets + the crates with their own `[lints]` table — is M2's
  scope).
- `test` — the full-workspace `cargo test --all-features` (the V8-gated
  hostile fixture + the full integration suite).
- `node` — the pnpm test suite (the conformance runner + the package
  tests).
- `conformance` — the TS conformance runner (C48 rides the `jshost-e2e`
  arm).
- `jshost-e2e` — the V8 isolate boot job (the hostile fixture's
  `#[ignore]`-gated pin executes here).
- `bench-gate` — the criterion bench job (the `perm` bench's release-mode
  numbers).
