// §V2-R01 C40 — Rust server function callable from the client (F08, binary only).
//
// Server-bound: a `*.server.rs` export is callable from the client over
// `/_oxide/fn/<id>` (the Rust function host, Binary/Docker only — the matrix
// refuses `rustFns` elsewhere). The check drives the live binary server — a
// Playwright spec.

import { ciBound } from './_server.ts'
import type { Assertion } from './types.ts'

const SPEC = 'tests/playwright/tests/conformance-c40-rust-server-fn.spec.ts'
const c40: Assertion = {
  id: 'C40',
  name: 'Rust server function callable from the client (F08, binary only)',
  capability: 'rust-fns',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'ci',
  ci: { spec: SPEC, job: 'playwright' },
  run: () => ciBound(SPEC, 'playwright'),
}
export default c40
