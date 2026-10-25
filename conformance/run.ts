// Conformance runner (§22): imports all four adapters, runs the single
// assertion set against each, prints a table, exits non-zero on any failure.
//   node conformance/run.ts
import { runSuite, type Assertion } from './suite.ts'
import { adapter as react, islandModules as reactIslands } from '../packages/react/src/index.ts'
import { adapter as vue, islandModules as vueIslands } from '../packages/vue/src/index.ts'
import { adapter as svelte, islandModules as svelteIslands } from '../packages/svelte/src/index.ts'
import { adapter as solid, islandModules as solidIslands } from '../packages/solid/src/index.ts'


/** Narrowing helper for noUncheckedIndexedAccess; throws instead of
 *  returning T | undefined. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`conformance: index ${i} out of bounds`)
  return v
}

const ADAPTERS: Array<[string, typeof react, Record<string, unknown>]> = [
  ['react', react, reactIslands],
  ['vue', vue, vueIslands],
  ['svelte', svelte, svelteIslands],
  ['solid', solid, solidIslands],
]

const all: Array<{ adapter: string; results: Assertion[] }> = []
for (const [name, adapter, islandChunks] of ADAPTERS) {
  const results = await runSuite(adapter, { islandChunks })
  all.push({ adapter: name, results })
}

// --- table -----------------------------------------------------------------
const names = mustEl(all, 0).results.map((r) => r.name)
const width = Math.max(...names.map((n) => n.length)) + 2
console.log('Oxide conformance suite (§22, D20) — one assertion set × four adapters\n')
console.log(`${'assertion'.padEnd(width)}${ADAPTERS.map(([n]) => n.padStart(9)).join('')}`)
let failures = 0
for (let i = 0; i < names.length; i++) {
  const cells = all.map(({ results }) => {
    const r = mustEl(results, i)
    if (!r.pass) failures++
    return (r.pass ? 'PASS' : 'FAIL').padStart(9)
  })
  console.log(`${mustEl(names, i).padEnd(width)}${cells.join('')}`)
}
console.log('')
for (const { adapter, results } of all) {
  const failed = results.filter((r) => !r.pass)
  console.log(`${adapter}: ${results.length - failed.length}/${results.length} passed`)
  for (const f of failed) console.log(`  ✖ ${f.name}: ${f.detail}`)
}
console.log('')
console.log(failures === 0 ? 'ALL GREEN — release not blocked (§22).' : `${failures} FAILURES — release blocked (§22).`)
process.exitCode = failures === 0 ? 0 : 1
