// Conformance gate (§22, D20): every assertion must pass for EVERY adapter —
// a release is blocked if any adapter regresses. One node:test per adapter;
// failures carry the failing assertion names + details.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { runSuite } from '../suite.ts'
import { adapter as react, islandModules as reactIslands } from '../../packages/react/src/index.ts'
import { adapter as vue, islandModules as vueIslands } from '../../packages/vue/src/index.ts'
import { adapter as svelte, islandModules as svelteIslands } from '../../packages/svelte/src/index.ts'
import { adapter as solid, islandModules as solidIslands } from '../../packages/solid/src/index.ts'

const ADAPTERS: Array<[string, typeof react, Record<string, unknown>]> = [
  ['react', react, reactIslands],
  ['vue', vue, vueIslands],
  ['svelte', svelte, svelteIslands],
  ['solid', solid, solidIslands],
]

for (const [name, adapter, islandChunks] of ADAPTERS) {
  test(`conformance: ${name} passes every §22 assertion`, async () => {
    const results = await runSuite(adapter, { islandChunks })
    const failed = results.filter((r) => !r.pass)
    const summary = `${name}: ${results.length - failed.length}/${results.length} assertions passed`
    assert.deepEqual(failed, [], `${summary}\n${failed.map((f) => `  ✖ ${f.name}: ${f.detail}`).join('\n')}`)
    assert.ok(results.length >= 18, `expected ≥18 assertions, got ${results.length} (§22 set drifted?)`)
  })
}
