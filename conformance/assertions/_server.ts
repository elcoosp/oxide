// V2-R01: the shared shape for ci-bound assertions. The node --test runner
// never executes a ci-bound assertion's `run` — the skip engine
// (`skip.ts`) short-circuits it with the recorded spec path + CI job. The
// `run` here is the defensive surface: if a caller invokes it directly it
// returns an EXPLAINED skip (the spec is written and registered; the runner
// just cannot drive a browser/server from the node process). The spec file
// itself is the execution surface that the named CI job runs.

import type { AssertionResult } from './types.ts'

/**
 * The explained skip a ci-bound assertion's `run` returns when invoked
 * directly. The skip engine records the same reason without calling `run`,
 * so this is the belt-and-braces path — never a TODO.
 */
export function ciBound(spec: string, job: string): AssertionResult {
  const reason = `ci-bound: executes via ${spec} in the ${job} CI job`
  return { ok: true, detail: reason, skipped: true, skipReason: reason }
}
