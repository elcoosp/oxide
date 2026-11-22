// §V2-R01 C39 — old build's server fn call succeeds inside the skew window (F10).
//
// Server-bound: the existing `tests/skew.sh` harness covers the full skew arc
// (A accepted in-window, B advertised, unknown 409+stale, expired 409, old
// generation's bytes frozen). C39's specific surface — an old-build server-fn
// call succeeding inside the window — is step 4 of that arc. The check rides
// the existing harness in the jshost-e2e job (no new spec: the wire is
// already pinned).

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/skew.sh'
const c39: Assertion = {
  id: 'C39',
  name: "old build's server fn call succeeds inside the skew window (F10)",
  capability: 'skew',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'jshost-e2e' },
  run: () => ciBound(SPEC, 'jshost-e2e'),
}
export default c39
