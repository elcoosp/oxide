# Benchmarks (spec §21)

Targets are tracked per the spec's benchmark method: reference apps (Hello, Blog,
Dashboard, Storefront) per framework, fixed hardware classes, warm+cold runs,
wrk/oha load generation, CI regression gate at 10% median vs the
`after-wave-b.json` snapshot.

- `crates/cli` `analyze` reports per-route JS bytes against `config.budgets`
  (§21: clientJs 150kb, routeJs 60kb) — exit code 1 on breach.
- Reference apps for the smoke harness live in `examples/` (smoke-app) and
  `conformance/atlas` (the §22 reference app doubles as the Blog-class workload).
- The full §21 table (dev start ≤ 300ms cached, HMR p95 ≤ 50ms, L0 hit p99 < 1ms,
  cold start ≤ 50ms with snapshots, etc.) requires the vendored engines documented
  in `docs/deviations.md` (V8 host, Rolldown); those rows are targets, not claims.

## Criterion microbenchmarks (V2-B09)

Wave B added a criterion harness over the V8-free core crates. Commands:

```bash
# benches (router, cache, dispatch, odf, render, server — every criterion
# target is feature-gated behind that crate's `bench-util`)
cargo bench -p oxide-fw-router -p oxide-fw-cache -p oxide-fw-dispatch \
  -p oxide-fw-odf -p oxide-fw-render -p oxide-fw-server \
  --features oxide-fw-router/bench-util --features oxide-fw-cache/bench-util \
  --features oxide-fw-dispatch/bench-util --features oxide-fw-odf/bench-util \
  --features oxide-fw-render/bench-util --features oxide-fw-server/bench-util

# summarize target/criterion into a JSON snapshot
python3 benchmarks/criterion_summary.py benchmarks/after-wave-b.json

# gate (the shape ci.yml's bench-gate runs): fail when any shared benchmark
# regresses >10% median vs the committed `after-wave-b.json` snapshot
python3 benchmarks/criterion_compare.py benchmarks/after-wave-b.json /tmp/criterion-fresh.json 10
```

Snapshots committed with the V2 Wave B milestone:

- `benchmarks/baseline-v01.json` — captured **before** B01/B02/B05/B06
  (the pre-change matcher, tag index and disk tier). A historical
  snapshot: the live criterion gate diffs against `after-wave-b.json`
  (the compare command above), not against this file.
- `benchmarks/after-wave-b.json` — captured after the milestone, at its
  HEAD. The two deliberate cost moves (`build/*`, `tag_insert/10000`)
  and the new ids (`key_construction/key_of`, `context_table_build/*`)
  are explained in `report.md` ("Criterion microbenchmarks").

The six-crate `cargo bench` command above is the full local sweep; CI's
bench-gate runs the two-crate shape (`cargo bench -p oxide-fw-router
-p oxide-fw-cache --features bench-util`) and holds its medians against
the committed `after-wave-b.json` at the 10% gate.

Allocation budgets live in `crates/router/tests/alloc_match.rs` (dhat,
own test binary: one profiler per process) — run by plain `cargo test`,
no feature needed.
