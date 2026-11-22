// §V2-R01 C30 — prerendered error page is served with its error status (A07).
//
// Server-bound: a prerendered error page (§10.5 sidecar) is served with the
// status the sidecar records, not a 200. The check drives the live/static
// server — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c30-prerendered-error.spec.ts'
const c30: Assertion = {
  id: 'C30',
  name: 'prerendered error page is served with its error status (A07)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c30
