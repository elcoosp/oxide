// §V2-R01 C25 — build bootstrap + x-oxide-build on /__data (A04).
//
// Server-bound: the HTML bootstrap advertises `build:` and every data-plane
// `/__data` response carries `x-oxide-build`. The check drives the live
// server (oxide start against the smoke app) over HTTP — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c25-build-bootstrap.spec.ts'
const c25: Assertion = {
  id: 'C25',
  name: 'HTML bootstrap contains build: and /__data carries x-oxide-build (A04)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c25
