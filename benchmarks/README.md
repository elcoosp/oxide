# Benchmarks (spec §21)

Targets are tracked per the spec's benchmark method: reference apps (Hello, Blog,
Dashboard, Storefront) per framework, fixed hardware classes, warm+cold runs,
wrk/oha load generation, CI regression gate at 5%.

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
# benches (router + cache; bench-util exposes the synthetic route generator)
cargo bench -p oxide-fw-router -p oxide-fw-cache \
  --features oxide-fw-router/bench-util --features oxide-fw-cache/bench-util

# summarize target/criterion into a JSON snapshot
python3 benchmarks/criterion_summary.py benchmarks/after-wave-b.json

# gate: fail when any shared benchmark regresses >5% vs a baseline snapshot
python3 benchmarks/criterion_compare.py benchmarks/baseline-v01.json benchmarks/after-wave-b.json 5
```

Snapshots committed with the V2 Wave B milestone:

- `benchmarks/baseline-v01.json` — captured **before** B01/B02/B05/B06
  (the pre-change matcher, tag index and disk tier). This is the file
  future Wave B work diffs against.
- `benchmarks/after-wave-b.json` — captured after the milestone, at its
  HEAD. The two deliberate cost moves (`build/*`, `tag_insert/10000`)
  and the new ids (`key_construction/key_of`, `context_table_build/*`)
  are explained in `report.md` ("Criterion microbenchmarks").

Allocation budgets live in `crates/router/tests/alloc_match.rs` (dhat,
own test binary: one profiler per process) — run by plain `cargo test`,
no feature needed.
