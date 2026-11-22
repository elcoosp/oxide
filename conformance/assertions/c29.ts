// §V2-R01 C29 — gated route's /__data is gated too (A05).
//
// Server-bound: a route whose middleware gates the HTML pipeline also gates
// its `/__data` (the data plane rides the same middleware chain). The check
// drives the live server — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c29-gated-route.spec.ts'
const c29: Assertion = {
  id: 'C29',
  name: "gated route's /__data is gated too (A05)",
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c29
