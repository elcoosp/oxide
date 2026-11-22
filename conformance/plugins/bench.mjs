// The V2-E05 plugin-compat bench (spec v0.2 §V2-E05): a cheap node bench
// over the assert evaluator + the report parser — "case resolution per
// plugin". Zero deps; runs V8-free. The existing criterion benches
// (`crates/plugin/benches/compat.rs`) stay green; this is the runner-side
// half.
//
// Measures:
//   - evaluateCase (glob + read + contains) over each recorded dist tree,
//   - parseReport (the structured oxide-plugin-load line + PLG-HOOK arm),
//   - renderMatrix (the verified-plugins table render).
// Reports per-iteration µs + ops/sec for a fixed iteration count.
//
// Run:  node conformance/plugins/bench.mjs

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { evaluateCase } from './evaluator.mjs';
import { parseReport, renderMatrix } from './report.mjs';
import { loadCase } from './schema.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const casesDir = join(here, 'cases');
const logsDir = join(here, 'logs');

const ITER = 5000;

/** @typedef {{ case: import('./schema.mjs').Case, name: string, log: string }} Loaded */
const loaded = readdirSync(casesDir)
  .filter((f) => f.endsWith('.json'))
  .sort()
  .map((f) => {
    const name = f.slice(0, -5);
    const c = loadCase(join(casesDir, f));
    const log = readFileSync(join(logsDir, name, 'build.log'), 'utf8');
    return { case: c, name, log };
  });

function bench(label, fn) {
  // warmup
  for (let i = 0; i < 200; i++) fn();
  const t0 = process.hrtime.bigint();
  for (let i = 0; i < ITER; i++) fn();
  const t1 = process.hrtime.bigint();
  const ns = Number(t1 - t0);
  const perUs = ns / ITER / 1000;
  const opsPerSec = 1e9 / (ns / ITER);
  console.log(
    `${label.padEnd(34)} ${perUs.toFixed(3).padStart(8)} µs/op   ${opsPerSec.toFixed(0).padStart(9)} ops/sec`,
  );
}

console.log(`V2-E05 plugin-compat bench — ${loaded.length} cases × ${ITER} iterations\n`);

// Per-case resolution: evaluateCase (the recorded dist) + parseReport (the
// recorded log) — the runner's hot path per plugin.
let evalTotal = 0;
let parseTotal = 0;
for (const l of loaded) {
  const distRoot = join(logsDir, l.name);
  bench(`evaluateCase  ${l.name}`, () => evaluateCase(l.case, distRoot));
  bench(`parseReport   ${l.name}`, () => parseReport(l.log));
}

// The matrix render (one render over all results).
const results = loaded.map((l) => {
  const r = parseReport(l.log);
  const report = r[l.case.plugin.split('@').slice(0, -1).join('@')] ?? r[l.name];
  const unsupported = report?.unsupportedHooks ?? [];
  return {
    plugin: l.case.plugin.split('@').slice(0, -1).join('@'),
    version: l.case.plugin.split('@').pop(),
    adapter: l.case.adapter,
    status: unsupported.length ? 'unsupported-hook' : 'pass',
    unsupportedHooks: unsupported,
    asserts: { passed: 1, failed: 0 },
  };
});
bench('renderMatrix (all 10)', () => renderMatrix(results));

console.log(`\n${loaded.length} cases; ${ITER} iterations each.`);
