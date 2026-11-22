// §V2-R01 C28 — ancestor middleware chain runs outer→inner + locals merge (A05).
//
// Server-bound: the middleware chain runs outer→inner and each ancestor's
// `locals` merge into the descendant's loader. The check drives the live
// server — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c28-middleware-chain.spec.ts'
const c28: Assertion = {
  id: 'C28',
  name: 'ancestor middleware chain runs outer to inner and locals merge (A05)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c28
