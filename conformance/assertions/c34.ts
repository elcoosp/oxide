// §V2-R01 C34 — why-dynamic reports line+snippet; --fix flips mode (F16).
//
// Server-bound (CLI surface): `oxide why <route>` reports the dynamic reason
// (line + snippet), and `oxide why <route> --fix` flips the render mode. The
// check drives the CLI over a fixture — a shell script run in the jshost-e2e
// job (which builds the CLI).

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/conformance-c34-why-dynamic.sh'
const c34: Assertion = {
  id: 'C34',
  name: 'why-dynamic reports line+snippet; --fix flips mode (F16)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'jshost-e2e' },
  run: () => ciBound(SPEC, 'jshost-e2e'),
}
export default c34
