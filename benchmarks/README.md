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
