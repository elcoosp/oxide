// V2-R01: the capability-skip engine. Wired to the EXISTING R02 matrix (the
// bridge in `capability.ts`) — never a second hand-maintained capability
// table. The rule (spec §V2-R01):
//   * a target NOT in the assertion's `targets` set → SKIP (target not
//     applicable);
//   * a ci-bound assertion → SKIP in the node runner, reason = the spec path
//     + the CI job that executes it (an EXPLAINED skip — the spec is written
//     and registered, the node runner just cannot drive a browser/server);
//   * the assertion's capability is `Unsupported` for the tested target →
//     SKIP, reason recorded (the matrix tells the truth — the §18 refusal
//     rule covers Unsupported at build time);
//   * `Degraded` → runs (the assertion accounts for the degradation; the
//     detail may carry the documented reason);
//   * `Full` (or a `null` baseline capability) → runs; a failure is a release
//     blocker.
//
// The runner exits non-zero on any Full/baseline failure AND on any
// UNEXPLAINED skip (a skip with no reason). Explained skips
// (target-not-applicable, ci-bound, capability-unsupported) do not block.

import { degradation, support } from './capability.ts'
import type { Assertion, Target } from './types.ts'

/** The skip decision for one (assertion, target) cell. */
export interface SkipDecision {
  skipped: boolean
  /** The recorded reason. Always present when `skipped` is true (an explained
   *  skip). The runner treats a skipped cell with no reason as UNEXPLAINED →
   *  exit non-zero. */
  reason?: string
}

/**
 * Decide whether `assertion` skips for `target`. Pure (no I/O); the runner
 * calls this before `run` so a skipped assertion's check never executes.
 */
export function skipStatus(a: Assertion, target: Target): SkipDecision {
  if (!a.targets.includes(target)) {
    return {
      skipped: true,
      reason: `target '${target}' is not in the assertion's target set (${a.targets.join(', ')})`,
    }
  }
  // The capability truth wins over the execution surface: a capability that
  // is Unsupported for the tested target is SKIPPED with the matrix reason
  // (the §18 refusal rule covers it at build time) — recorded before the
  // ci-bound dimension so the table shows WHY (capability), not just where
  // (CI job).
  if (a.capability !== null) {
    const s = support(target, a.capability)
    if (s === 'unsupported') {
      const deg = degradation(target, a.capability)
      return {
        skipped: true,
        reason: deg
          ? `capability '${a.capability}' is Unsupported for target '${target}' (degradation: ${deg})`
          : `capability '${a.capability}' is Unsupported for target '${target}' (the §18 refusal rule covers it at build time)`,
      }
    }
  }
  // A ci-bound assertion executes in a CI job (a Playwright spec / shell
  // script), never in the node --test runner. The node runner records the
  // skip with the spec path + job name — the execution surface is recorded,
  // not deferred.
  if (a.surface === 'ci' && a.ci) {
    return {
      skipped: true,
      reason: `ci-bound: executes via ${a.ci.spec} in the ${a.ci.job} CI job`,
    }
  }
  return { skipped: false }
}
