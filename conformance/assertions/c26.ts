// §V2-R01 C26 — stale build 409 + x-oxide-build-stale (A04).
//
// Server-bound: an unknown/expired build id on the data plane answers 409
// with `x-oxide-build-stale: true`. The check drives the live server over
// HTTP — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c26-build-stale.spec.ts'
const c26: Assertion = {
  id: 'C26',
  name: 'stale build header gets 409 with x-oxide-build-stale (A04)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c26
