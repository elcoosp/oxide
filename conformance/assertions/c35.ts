// §V2-R01 C35 — live route updates after revalidate within 200ms (F05).
//
// Server-bound: a live route re-runs its loader when a tag it depends on is
// revalidated and the open tab receives the update within 200 ms (the §V2.2
// G2 target). The check drives the live hub over the SSE endpoint — a
// Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c35-live-revalidate.spec.ts'
const c35: Assertion = {
  id: 'C35',
  name: 'live route updates after revalidate within 200ms (F05)',
  capability: 'live',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c35
