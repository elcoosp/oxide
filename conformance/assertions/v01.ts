// V2-R01: the v0.1 assertion set migrated into the registry. Each entry wraps
// a `V01_CHECKS` check (the pre-R01 bodies, verbatim — `runSuite` still runs
// them for the `conformance.test.ts` guard) into a `§V2-R01` assertion
// descriptor the new runner dispatches through the same skip engine as the
// C25–C47 set.
//
// The v0.1 checks are baseline contract assertions: no capability gating
// (they pin the §7/§22 contract every adapter owes every target), so
// `capability: null` — they run on every cell, and a failure is a release
// blocker (the skip engine treats a `null` capability as Full).

import { buildCheckCtx, V01_CHECKS } from '../suite.ts'
import type { Assertion, RunContext } from './types.ts'

/** The four adapters every v0.1 assertion runs against (§22: one set × all 4). */
const ALL_ADAPTERS: Assertion['adapters'] = ['react', 'vue', 'svelte', 'solid']

/** The runner's target set (§V2-R01 impl note: {binary, node}). The v0.1
 *  checks are client-runtime and target-agnostic — they run on both. */
const RUNNER_TARGETS: Assertion['targets'] = ['binary', 'node']

/**
 * The v0.1 assertions as registry descriptors. `id` is the stable kebab
 * spelling the table prints; the registry keys on it and `run.ts`'s
 * `--adapter`/`--target` flags filter the cross product by it.
 */
export const V01_ASSERTIONS: readonly Assertion[] = V01_CHECKS.map((entry) => ({
  id: entry.id,
  name: entry.name,
  capability: null,
  adapters: ALL_ADAPTERS,
  targets: RUNNER_TARGETS,
  surface: 'client',
  async run(ctx: RunContext) {
    // Build the CheckCtx the v0.1 check closes over from the registry's
    // RunContext — the same shape `runSuite` builds, so the check sees an
    // identical environment whether dispatched by the v0.1 guard or the R01
    // runner.
    const c = buildCheckCtx(ctx.adapter, { islandChunks: ctx.islandChunks })
    const detail = await entry.fn(c)
    return { ok: true, detail }
  },
}))
