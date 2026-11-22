// §V2-R01 C48 — a route declaring permissions denies a fetch/env read
// outside its grant (F19).
//
// Server-bound: the V8 isolate's `op_oxide_fetch` / `op_oxide_env_get`
// boundary checks raise `PRM-001` when a route's declared `permissions`
// grant denies the host / env name. The check drives the V8 host over a
// real isolate — the `jshost-e2e` CI job (the workspace job that boots
// isolates). The hostile fixture (a `setTimeout` created outside the
// request context) is the Model 1 → Model 2 closure pin, registered in
// `crates/jshost/tests/perm_tdd.rs`.
//
// On Node targets the enforcement is advisory (D35: a structured warn,
// never a hard block) — the matrix records `permissions` as `degraded`
// on `node`/`deno`/`bun`; the skip engine runs the assertion (Degraded
// runs) but a failure is non-blocking (the runner's `isBlocker` reads
// the cell).

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'crates/jshost/tests/perm_tdd.rs (the hostile fixture + the PermGuard pins)'
const c48: Assertion = {
  id: 'C48',
  name: 'a route declaring permissions denies a fetch/env read outside its grant (F19)',
  capability: 'permissions',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'jshost-e2e' },
  run: () => ciBound(SPEC, 'jshost-e2e'),
}
export default c48
