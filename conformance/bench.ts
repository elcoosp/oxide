// §V2-R01 bench: a cheap node bench over the registry dispatch + skip engine
// (assertion resolution per adapter×target cell). The "before" is the
// pre-R01 dispatch shape — iterate the assertion set per adapter, no target
// dimension, no skip decision (the v0.1 `run.ts` looped `runSuite` once per
// adapter). The "after" is the R01 dispatch — for every (adapter,target)
// cell, `skipStatus` over every assertion (a matrix-bridge lookup per cell).
//
// The bench measures DISPATCH + SKIP resolution only (no `run()` execution):
// the cost the new target dimension + capability-skip engine adds over the
// v0.1 per-adapter loop. Run with `node conformance/bench.ts`.

import { ALL_ASSERTIONS } from './assertions/index.ts'
import { skipStatus } from './assertions/skip.ts'
import { RUNNER_ADAPTERS, RUNNER_TARGETS } from './assertions/types.ts'

const N = 5_000

function nsNow(): bigint {
  return process.hrtime.bigint()
}
function fmt(ns: bigint): string {
  return `${(Number(ns) / 1e6).toFixed(3)} ms`
}

// The cross product the runner exercises.
const cells: Array<[string, string]> = []
for (const t of RUNNER_TARGETS) for (const a of RUNNER_ADAPTERS) cells.push([a, t])

// --- before: the pre-R01 dispatch (per adapter, no target, no skip) -------
// The v0.1 `run.ts` ran `runSuite` once per adapter; its dispatch was a flat
// iteration over the assertion set with no per-cell decision.
let beforeTotal = 0n
for (const _cell of cells) {
  void _cell
  const t0 = nsNow()
  for (let i = 0; i < N; i++) {
    for (const _a of ALL_ASSERTIONS) {
      void _a
    }
  }
  beforeTotal += nsNow() - t0
}

// --- after: the R01 dispatch (per (adapter,target) cell, skipStatus) -------
// For every cell, `skipStatus` over every assertion — a matrix-bridge lookup
// (the capability-support cell) per assertion.
let afterTotal = 0n
for (const [a, t] of cells) {
  const t0 = nsNow()
  for (let i = 0; i < N; i++) {
    for (const assertion of ALL_ASSERTIONS) {
      const dec = skipStatus(assertion, t)
      void dec
      void a
    }
  }
  afterTotal += nsNow() - t0
}

const perCellBefore = beforeTotal / BigInt(cells.length)
const perCellAfter = afterTotal / BigInt(cells.length)
const perAssertionAfter = afterTotal / BigInt(cells.length * ALL_ASSERTIONS.length * N)

console.log('Oxide conformance §V2-R01 bench — registry dispatch + skip engine')
console.log(`  assertions: ${ALL_ASSERTIONS.length}`)
console.log(`  cells: ${cells.length} (=${RUNNER_ADAPTERS.length} adapters × ${RUNNER_TARGETS.length} targets)`)
console.log(`  iterations per cell: ${N}`)
console.log('')
console.log(`  before (per-adapter, no skip):  ${fmt(perCellBefore)}/cell × ${N} iters`)
console.log(`  after  (per-cell × skipStatus): ${fmt(perCellAfter)}/cell × ${N} iters`)
console.log(`  after  per-assertion per-iter:  ${Number(perAssertionAfter)} ns`)
console.log('')
console.log(`  total before: ${fmt(beforeTotal)}   total after: ${fmt(afterTotal)}`)
console.log(`  the skip engine adds ~${(Number(perCellAfter - perCellBefore) / 1e6).toFixed(3)} ms/cell over the v0.1 loop`)
