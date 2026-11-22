// §V2-R01 C44 — record then replay is byte-identical (F15).
//
// Server-bound: a recorded request/response pair replays byte-identical
// (the interaction-replay contract). The check drives the live server's
// record+replay mode — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c44-record-replay.spec.ts'
const c44: Assertion = {
  id: 'C44',
  name: 'record then replay is byte-identical (F15)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c44
