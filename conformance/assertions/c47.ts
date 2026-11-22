// §V2-R01 C47 — HMR keeps input state across a component edit (E01).
//
// Server-bound: a component edit over HMR preserves the in-flight input
// state (the §V2-E01 state-preservation substrate). The check drives the dev
// server's HMR over a real browser — a Playwright spec (the existing
// `hmr-*.spec.ts` family's pattern).

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c47-hmr-input-state.spec.ts'
const c47: Assertion = {
  id: 'C47',
  name: 'HMR keeps input state across a component edit (E01)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c47
