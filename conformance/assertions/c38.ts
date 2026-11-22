// §V2-R01 C38 — brownout sheds low-priority and keeps critical routes (F12).
//
// Server-bound: under brownout (queue depth over threshold), low-priority
// routes shed (503) while critical routes keep serving (200). The check
// drives the live server's brownout mode — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c38-brownout.spec.ts'
const c38: Assertion = {
  id: 'C38',
  name: 'brownout sheds low-priority and keeps critical routes (F12)',
  capability: 'brownout',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c38
