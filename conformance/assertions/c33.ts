// §V2-R01 C33 — experiment variants get separate L0 entries (F04).
//
// Server-bound: two experiment buckets for one route get distinct L0 cache
// entries (the variant rides the cache key); a no-experiment route keeps the
// old key. The check drives the live server and observes the variant split
// via the served content + the cache header — a Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c33-experiment-variants.spec.ts'
const c33: Assertion = {
  id: 'C33',
  name: 'experiment variants get separate L0 entries (F04)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c33
