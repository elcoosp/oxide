# Oxide conformance suite (§22 + §V2-R01)

One assertion set run against all four framework adapters (§22, D20), extended
by the §V2-R01 registry: the v0.1 set migrated into a registry + every C25–C47
feature assertion, dispatched through a capability-skip engine wired to the
R02 matrix. A release is blocked if any adapter regresses on a Full
capability; an Unsupported capability is SKIPPED with a recorded reason.

Zero runtime dependencies — runs on Node 24 type stripping against a
hand-rolled fake DOM.

## Run

```sh
# The full cross product {react,vue,svelte,solid} × {binary, node}:
node conformance/run.ts

# One cell (the consumer example — prints the capability-aware table):
node conformance/run.ts --adapter react --target binary
```

The table prints PASS/FAIL/SKIP(reason) per (assertion × cell). The runner
exits non-zero on any Full/baseline failure and on any UNEXPLAINED skip.

## The registry (§V2-R01)

Every assertion is a descriptor in `conformance/assertions/`:

- `conformance/assertions/types.ts` — the `Assertion` shape (`{ id,
  capability, adapters, targets, surface, run(ctx) -> { ok, detail } }`).
- `conformance/assertions/v01.ts` — the migrated v0.1 set (§22 contract
  assertions; their checks are unchanged — `runSuite` still runs them for the
  `conformance.test.ts` guard).
- `conformance/assertions/c25.ts … c47.ts` — the §V2-R01 feature assertions,
  one file per spec id. Client-runtime assertions (C31/C32/C36/C37/C41/C42/
  C43/C45/C46) run NOW via `node --test` against the fake DOM; server-bound
  assertions (C25–C30/C33/C34/C35/C38/C39/C40/C44/C47) are registry entries
  whose `ci` field names the Playwright spec / shell script + the CI job that
  executes them.
- `conformance/assertions/index.ts` — collects them into `ALL_ASSERTIONS`.
- `conformance/assertions/capability.ts` — the TS bridge over the R02 matrix
  (`conformance/matrix.json`, generated from
  `oxide_adapters::capability_matrix` by `cargo run -p oxide-fw-adapters
  --example dump_matrix`; the drift guard is
  `crates/adapters/tests/matrix_json.rs`).
- `conformance/assertions/skip.ts` — the capability-skip engine: Unsupported
  → SKIP(reason), Degraded → runs with the documented degradation, Full →
  runs (a failure is a release blocker). ci-bound assertions skip in the node
  runner with the spec + CI job recorded.

## Local vs CI

- **Local (V8-free):** `node --test 'test/*.test.ts'` runs the registry +
  skip engine + every client-runtime assertion against the fake DOM. The
  server-bound assertions skip ci-bound (explained) — their specs run in CI.
- **CI:** the `conformance` job runs `pnpm -C conformance test` (the node
  suite); the `playwright` job runs the server-bound Playwright specs; the
  `jshost-e2e` job runs `tests/skew.sh` (C39) and
  `tests/conformance-c34-why-dynamic.sh` (C34). The `{4 adapters} × {binary,
  node}` cross product rides these jobs.

## The bench

```sh
node conformance/bench.ts
```

A cheap bench over the registry dispatch + skip engine (assertion resolution
per adapter×target cell), with before/after numbers. The before is the
pre-R01 per-adapter loop; the after is the per-cell × `skipStatus` dispatch.
