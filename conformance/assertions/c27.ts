// §V2-R01 C27 — parent-dependent and independent loads overlap (A06).
//
// Server-bound: parallel layout loads (§9.2) — a parent route's data and its
// children's data load concurrently (timing-observable). The check drives the
// live server and observes load overlap — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c27-parallel-loads.spec.ts'
const c27: Assertion = {
  id: 'C27',
  name: 'parent-dependent and independent loads overlap (A06)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c27
