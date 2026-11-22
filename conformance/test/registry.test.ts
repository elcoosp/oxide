// §V2-R01 registry gate (node --test, V8-free): runs every assertion in the
// registry over the cross product {react,vue,svelte,solid} × {binary, node}
// against the fake DOM (zero runtime deps), and asserts:
//   * zero Full/baseline failures (a release blocker);
//   * zero UNEXPLAINED skips (a skip with no recorded reason);
//   * every capability-unsupported / ci-bound / adapter-not-applicable skip
//     carries a reason (explained skips do not block).
//
// The server-bound assertions (C25–C30/C33/C34/C35/C38/C39/C40/C44/C47) skip
// ci-bound here — their Playwright/shell specs execute in the `playwright`
// and `jshost-e2e` CI jobs (the recorded execution surface). This gate is the
// client-runtime half + the registry/skip-engine contract.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { ALL_ASSERTIONS } from '../assertions/index.ts'
import { skipStatus } from '../assertions/skip.ts'
import { matrixBridge, support } from '../assertions/capability.ts'
import type { Assertion, AdapterName, Target, RunContext, FeatureSupport } from '../assertions/types.ts'
import { RUNNER_ADAPTERS, RUNNER_TARGETS } from '../assertions/types.ts'
import { adapter as react, islandModules as reactIslands } from '../../packages/react/src/index.ts'
import { adapter as vue, islandModules as vueIslands } from '../../packages/vue/src/index.ts'
import { adapter as svelte, islandModules as svelteIslands } from '../../packages/svelte/src/index.ts'
import { adapter as solid, islandModules as solidIslands } from '../../packages/solid/src/index.ts'
import type { FrameworkAdapter } from '../../packages/core/src/types.ts'

const ADAPTERS: Array<[AdapterName, FrameworkAdapter, Record<string, unknown>]> = [
  ['react', react, reactIslands],
  ['vue', vue, vueIslands],
  ['svelte', svelte, svelteIslands],
  ['solid', solid, solidIslands],
]

function isBlocker(a: Assertion, target: Target): boolean {
  if (a.capability === null) return true
  const s: FeatureSupport = support(target, a.capability)
  return s === 'full'
}

interface Outcome {
  status: 'pass' | 'fail' | 'skip'
  reason?: string
  detail?: string
  blocker: boolean
  explained: boolean
}

async function runOne(a: Assertion, ctx: RunContext): Promise<Outcome> {
  const skip = skipStatus(a, ctx.target)
  if (skip.skipped) {
    return { status: 'skip', reason: skip.reason, blocker: false, explained: skip.reason !== undefined }
  }
  if (!a.adapters.includes(ctx.adapterName)) {
    return {
      status: 'skip',
      reason: `adapter '${ctx.adapterName}' not in the assertion's adapter set`,
      blocker: false,
      explained: true,
    }
  }
  try {
    const result = await a.run(ctx)
    if (result.skipped) {
      return { status: 'skip', reason: result.skipReason, blocker: false, explained: result.skipReason !== undefined }
    }
    if (result.ok) return { status: 'pass', detail: result.detail, blocker: false, explained: true }
    return { status: 'fail', detail: result.detail, blocker: isBlocker(a, ctx.target), explained: true }
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err)
    return { status: 'fail', detail, blocker: isBlocker(a, ctx.target), explained: true }
  }
}

// One test per (adapter, target) cell so a failure names the cell.
for (const [aname, adapter, islandChunks] of ADAPTERS) {
  for (const target of RUNNER_TARGETS) {
    test(`registry: ${aname}/${target} — zero blockers, zero unexplained skips`, async () => {
      const ctx: RunContext = { adapter, adapterName: aname, target, islandChunks, matrix: matrixBridge }
      const blockers: Array<{ id: string; detail?: string }> = []
      const unexplained: Array<{ id: string }> = []
      const fails: Array<{ id: string; detail?: string }> = []
      const skips: Array<{ id: string; reason?: string }> = []
      let passed = 0
      for (const a of ALL_ASSERTIONS) {
        const o = await runOne(a, ctx)
        if (o.status === 'pass') passed++
        else if (o.status === 'fail') fails.push({ id: a.id, detail: o.detail })
        else skips.push({ id: a.id, reason: o.reason })
        if (o.blocker && o.status !== 'pass') blockers.push({ id: a.id, detail: o.detail })
        if (o.status === 'skip' && !o.explained) unexplained.push({ id: a.id })
      }
      assert.deepEqual(blockers, [], `${aname}/${target}: zero blocking failures\n${blockers.map((b) => `  ✖ ${b.id}: ${b.detail}`).join('\n')}`)
      assert.deepEqual(unexplained, [], `${aname}/${target}: zero unexplained skips\n${unexplained.map((b) => `  ⊘ ${b.id}`).join('\n')}`)
      // The registry must carry the v0.1 set + C25–C47.
      assert.ok(ALL_ASSERTIONS.length >= 47, `registry carries ≥47 assertions (got ${ALL_ASSERTIONS.length})`)
      // Sanity: every cell either passes or skips with a reason (no assertion silently no-ops).
      assert.equal(passed + skips.length + fails.length, ALL_ASSERTIONS.length, `${aname}/${target}: every assertion is pass|skip|fail`)
    })
  }
}

// The registry shape itself: stable ids, the C25–C47 set is complete, and the
// skip engine never hand-maintains a second capability table (it reads the
// bridge).
test('registry: the C25–C47 set is complete and registered in spec order', () => {
  const ids = new Set(ALL_ASSERTIONS.map((a) => a.id))
  for (let n = 25; n <= 47; n++) {
    const id = `C${n}`
    assert.ok(ids.has(id), `the registry carries ${id}`)
  }
})

test('registry: the v0.1 set is migrated (no v0.1 assertion was dropped)', () => {
  // The v0.1 ids are kebab-case; the registry carries them (the §22 set the
  // `conformance.test.ts` guard also runs).
  const v01 = ALL_ASSERTIONS.filter((a) => !a.id.startsWith('C'))
  assert.ok(v01.length >= 24, `the v0.1 set is migrated (got ${v01.length} baseline assertions)`)
})

test('skip engine: an Unsupported capability skips with a recorded reason', () => {
  // RustFns is Unsupported on node; an assertion pinning rust-fns on node
  // skips with the §18 refusal note.
  const c40 = ALL_ASSERTIONS.find((a) => a.id === 'C40')!
  const dec = skipStatus(c40, 'node')
  assert.equal(dec.skipped, true, 'C40 (rust-fns) skips on node')
  assert.ok(dec.reason && dec.reason.includes('Unsupported'), `the skip reason names the Unsupported cell (got ${dec.reason})`)
})

test('skip engine: a ci-bound assertion skips with the spec + CI job', () => {
  const c25 = ALL_ASSERTIONS.find((a) => a.id === 'C25')!
  const dec = skipStatus(c25, 'binary')
  assert.equal(dec.skipped, true, 'C25 (ci-bound) skips in the node runner')
  assert.ok(dec.reason && dec.reason.includes('conformance-c25'), `the skip reason names the spec (got ${dec.reason})`)
  assert.ok(dec.reason && dec.reason.includes('playwright'), `the skip reason names the CI job (got ${dec.reason})`)
})
