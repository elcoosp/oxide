// Conformance runner (§V2-R01): imports the assertion registry, runs the
// cross product {react,vue,svelte,solid} × {binary, node}, and prints a
// PASS/FAIL/SKIP(reason) table. The skip engine wires the EXISTING R02
// capability matrix (the JSON bridge over `oxide_adapters::capability_matrix`)
// — an assertion whose capability is Unsupported for the tested target is
// SKIPPED with a recorded reason; a Full capability that fails is a release
// blocker. Exits non-zero on any Full/baseline failure and on any
// UNEXPLAINED skip.
//
//   node conformance/run.ts                                 # full cross product
//   node conformance/run.ts --adapter react --target binary # one cell
import { ALL_ASSERTIONS } from './assertions/index.ts'
import { skipStatus } from './assertions/skip.ts'
import { matrixBridge, support } from './assertions/capability.ts'
import type { Assertion, AdapterName, Target, RunContext, FeatureSupport } from './assertions/types.ts'
import { RUNNER_ADAPTERS, RUNNER_TARGETS } from './assertions/types.ts'
import { adapter as react, islandModules as reactIslands } from '../packages/react/src/index.ts'
import { adapter as vue, islandModules as vueIslands } from '../packages/vue/src/index.ts'
import { adapter as svelte, islandModules as svelteIslands } from '../packages/svelte/src/index.ts'
import { adapter as solid, islandModules as solidIslands } from '../packages/solid/src/index.ts'
import type { FrameworkAdapter } from '../packages/core/src/types.ts'

/** The four adapters + their build-generated island chunk maps. */
const ADAPTERS: Array<[AdapterName, FrameworkAdapter, Record<string, unknown>]> = [
  ['react', react, reactIslands],
  ['vue', vue, vueIslands],
  ['svelte', svelte, svelteIslands],
  ['solid', solid, solidIslands],
]

// --- flag parsing (--adapter <name>, --target <name>; repeatable) ----------
function parseFlags(argv: string[]): { adapters: AdapterName[]; targets: Target[] } {
  const adapters = new Set<AdapterName>()
  const targets = new Set<Target>()
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--adapter' && argv[i + 1]) {
      adapters.add(argv[++i] as AdapterName)
    } else if (a === '--target' && argv[i + 1]) {
      targets.add(argv[++i] as Target)
    }
  }
  return {
    adapters: adapters.size === 0 ? [...RUNNER_ADAPTERS] : [...RUNNER_ADAPTERS].filter((a) => adapters.has(a)),
    targets: targets.size === 0 ? [...RUNNER_TARGETS] : [...RUNNER_TARGETS].filter((t) => targets.has(t)),
  }
}

/** A Full/baseline failure is a release blocker; a Degraded failure is
 *  printed but non-blocking (the degradation is documented). */
function isBlocker(a: Assertion, target: Target): boolean {
  if (a.capability === null) return true // baseline contract assertion
  const s: FeatureSupport = support(target, a.capability)
  return s === 'full'
}

interface CellOutcome {
  status: 'pass' | 'fail' | 'skip'
  reason?: string
  detail?: string
  blocker: boolean
}

async function runCell(a: Assertion, ctx: RunContext): Promise<CellOutcome> {
  const skip = skipStatus(a, ctx.target)
  if (skip.skipped) {
    // An explained skip (capability-unsupported, ci-bound, target-not-applicable)
    // is never a blocker. A skip with NO reason is unexplained → blocker.
    return { status: 'skip', reason: skip.reason, blocker: skip.reason === undefined }
  }
  try {
    const result = await a.run(ctx)
    if (result.skipped) {
      return { status: 'skip', reason: result.skipReason, blocker: result.skipReason === undefined }
    }
    if (result.ok) return { status: 'pass', detail: result.detail, blocker: false }
    // A FAIL on a Full/baseline capability blocks; on Degraded it is loud but
    // non-blocking (the matrix already told the truth about the degradation).
    return { status: 'fail', detail: result.detail, blocker: isBlocker(a, ctx.target) }
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err)
    return { status: 'fail', detail, blocker: isBlocker(a, ctx.target) }
  }
}

const { adapters: chosenAdapters, targets: chosenTargets } = parseFlags(process.argv.slice(2))

/** The (adapter, target) cells the runner exercises. */
const cells: Array<[AdapterName, Target]> = []
for (const t of chosenTargets) for (const a of chosenAdapters) cells.push([a, t])

const colLabel = (a: AdapterName, t: Target): string => `${a}/${t.slice(0, 3)}`

// --- run every assertion × every cell --------------------------------------
type Grid = Map<string, CellOutcome> // key = `${assertionId}|${adapter}/${target}`
const grid: Grid = new Map()
for (const [aname, adapter, islandChunks] of ADAPTERS.filter(([n]) => chosenAdapters.includes(n))) {
  for (const target of chosenTargets) {
    const ctx: RunContext = { adapter, adapterName: aname, target, islandChunks, matrix: matrixBridge }
    for (const a of ALL_ASSERTIONS) {
      if (!a.adapters.includes(aname)) {
        // The assertion does not apply to this adapter (e.g. C31 is react-host-page).
        grid.set(`${a.id}|${aname}/${target}`, {
          status: 'skip',
          reason: `adapter '${aname}' not in the assertion's adapter set (${a.adapters.join(', ')})`,
          blocker: false,
        })
        continue
      }
      const outcome = await runCell(a, ctx)
      grid.set(`${a.id}|${aname}/${target}`, outcome)
    }
  }
}

// --- table -----------------------------------------------------------------
const ids = ALL_ASSERTIONS.map((a) => a.id)
const idWidth = Math.max(...ids.map((n) => n.length)) + 2
const colW = Math.max(...cells.map(([a, t]) => colLabel(a, t).length), 9) + 1
const symbol = (o: CellOutcome): string => (o.status === 'pass' ? 'P' : o.status === 'fail' ? 'F' : 'S')
const blockerSym = (o: CellOutcome): string => (o.blocker && o.status !== 'pass' ? '!' : '')

console.log('Oxide conformance suite (§V2-R01) — registry × {adapter}×{target}\n')
console.log(`${'assertion'.padEnd(idWidth)}${cells.map(([a, t]) => colLabel(a, t).padEnd(colW)).join('')}`)
let blockers = 0
let unexplainedSkips = 0
for (const a of ALL_ASSERTIONS) {
  const row = cells.map(([an, t]) => {
    const o = grid.get(`${a.id}|${an}/${t}`)!
    if (o.blocker && o.status === 'fail') blockers++
    if (o.status === 'skip' && o.reason === undefined) unexplainedSkips++
    return `${symbol(o)}${blockerSym(o)}`.padEnd(colW)
  })
  console.log(`${a.id.padEnd(idWidth)}${row.join('')}`)
}
console.log('')
console.log('legend: P=PASS  F=FAIL  S=SKIP  !=release blocker (Full/baseline fail or unexplained skip)\n')

// --- details: failures + skips with reasons --------------------------------
for (const [aname, target] of cells) {
  const fails: Array<{ a: Assertion; o: CellOutcome }> = []
  const skips: Array<{ a: Assertion; o: CellOutcome }> = []
  for (const a of ALL_ASSERTIONS) {
    const o = grid.get(`${a.id}|${aname}/${target}`)!
    if (o.status === 'fail') fails.push({ a, o })
    if (o.status === 'skip') skips.push({ a, o })
  }
  const passed = ALL_ASSERTIONS.filter((a) => grid.get(`${a.id}|${aname}/${target}`)!.status === 'pass').length
  console.log(`${aname}/${target}: ${passed}/${ALL_ASSERTIONS.length} passed, ${skips.length} skipped, ${fails.length} failed`)
  for (const { a, o } of fails) console.log(`  ✖ ${a.id}: ${o.detail}`)
  for (const { a, o } of skips) {
    if (o.reason) console.log(`  ⊘ ${a.id}: ${o.reason}`)
  }
}
console.log('')
const verdict =
  blockers === 0 && unexplainedSkips === 0
    ? 'ALL GREEN — release not blocked (§V2-R01).'
    : `${blockers} BLOCKING FAILURES + ${unexplainedSkips} UNEXPLAINED SKIPS — release blocked (§V2-R01).`
console.log(verdict)
process.exitCode = blockers === 0 && unexplainedSkips === 0 ? 0 : 1
