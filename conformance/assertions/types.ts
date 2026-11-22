// V2-R01: the assertion registry seam (spec §V2-R01). Each assertion is a
// descriptor the runner resolves against a (adapter, target) cell; the skip
// engine (`skip.ts`) decides per cell whether it runs, skips
// (capability-unsupported or ci-bound), or blocks on failure (a Full
// capability that fails is a release blocker).
//
// The shape mirrors the spec's "`conformance/assertions/<id>.ts`, each
// exporting { id, capability, adapters, targets, run(input) -> { ok, detail }
// }" — extended with `surface` (the local-vs-CI split, §V2-R01 impl note) and
// `ci` (the recorded execution surface for server-bound assertions).

import type { FrameworkAdapter } from '../../packages/core/src/types.ts'

/** The deploy targets the matrix declares (mirrors `Target::as_str`). The
 *  cross product the runner exercises is `{react,vue,svelte,solid} ×
 *  {binary, node}` (§V2-R01 impl note); the wider set is the matrix's row
 *  vocabulary so a capability skip reasons over any target. */
export type Target =
  | 'binary' | 'node' | 'static' | 'docker' | 'cloudflare' | 'cloudflare-pages'
  | 'lambda' | 'vercel' | 'vercel-functions' | 'netlify' | 'deno' | 'bun'

/** The four framework adapters the suite runs against (§22). */
export type AdapterName = 'react' | 'vue' | 'svelte' | 'solid'

/** The capability ids the R02 matrix declares (mirrors `Capability::as_str`).
 *  A `null` capability on an assertion means a baseline contract assertion
 *  that runs on every target (no skip). */
export type CapabilityId =
  | 'l0-cache' | 'isr' | 'sockets' | 'image-optimization' | 'native-rules'
  | 'snapshots' | 'tls' | 'live' | 'mixed-islands' | 'resume' | 'skew'
  | 'delta-deploy' | 'rust-fns' | 'search' | 'offline' | 'brownout'
  | 'permissions' | 'attestation'

/** Where an assertion executes: the node runner (`client`, V8-free fake DOM)
 *  or a CI job (`ci`, a registered Playwright spec / shell script). */
export type Surface = 'client' | 'ci'

/** The R02 feature-support cell (mirrors `FeatureSupport`'s serde rename). */
export type FeatureSupport = 'full' | 'degraded' | 'unsupported'

/** One assertion's per-cell result. */
export interface AssertionResult {
  ok: boolean
  detail: string
  /** A self-explained skip (a client assertion that found its feature
   *  unadvertised at runtime). A `run` that throws without setting `skipped`
   *  is an UNEXPLAINED skip → the runner exits non-zero (§V2-R01: "zero
   *  unexplained skips"). */
  skipped?: boolean
  skipReason?: string
}

/** The matrix bridge handed to every `run` as `ctx.matrix` — the TS side of
 *  the R02 substrate (`capability.ts` implements it over the generated JSON). */
export interface MatrixBridge {
  support(target: Target, cap: CapabilityId): FeatureSupport
  degradation(target: Target, cap: CapabilityId): string | null
}

/** The context a `run` closes over: the adapter under test, the deploy
 *  target, the build's island chunk map, and the matrix bridge (so a Degraded
 *  assertion can read its own degradation reason into the detail). */
export interface RunContext {
  adapter: FrameworkAdapter
  adapterName: AdapterName
  target: Target
  islandChunks?: Record<string, unknown>
  matrix: MatrixBridge
}

/** The recorded execution surface for a ci-bound assertion (§V2-R01: "a
 *  CI-bound assertion is a written, registered spec with a recorded execution
 *  surface, not a TODO"). */
export interface CiSurface {
  /** The spec file that implements the check (e.g. a Playwright `.spec.ts`
   *  under `tests/playwright/tests/`, or a shell script under `tests/`). */
  spec: string
  /** The CI job name that executes it (e.g. `playwright`, `jshost-e2e`). */
  job: string
}

/** One assertion descriptor. The registry collects these; `run.ts` runs the
 *  cross product `{react,vue,svelte,solid} × {binary, node}` and prints a
 *  PASS/FAIL/SKIP(reason) table. */
export interface Assertion {
  /** The stable spelling the table prints and the registry keys on. The v0.1
   *  set carries kebab ids; the v0.2 set carries the spec's C25–C47 ids. */
  id: string
  /** A human label for the table row and the failure detail. */
  name: string
  /** The R02 capability this assertion pins, or `null` for a baseline
   *  contract assertion (runs on every target, no capability skip). */
  capability: CapabilityId | null
  /** The framework set (the spec's "all 4" or "react host page"). */
  adapters: readonly AdapterName[]
  /** The deploy set the assertion applies to. */
  targets: readonly Target[]
  surface: Surface
  /** For ci-bound assertions: the spec path + CI job (required when
   *  `surface === 'ci'`). */
  ci?: CiSurface
  /** The check. A throw → the runner treats it as FAIL (`ok=false`) with the
   *  thrown message as the detail. Returning `{ skipped: true, skipReason }`
   *  is the only path to an explained runtime skip. */
  run(ctx: RunContext): Promise<AssertionResult> | AssertionResult
}

/** The cross product the runner exercises (§V2-R01 impl note). */
export const RUNNER_ADAPTERS: readonly AdapterName[] = ['react', 'vue', 'svelte', 'solid']
export const RUNNER_TARGETS: readonly Target[] = ['binary', 'node']
