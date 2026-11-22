// §V2-R01 C36 — patch payload applies; mismatched base falls back to full (F06).
//
// Pins `packages/core/src/client/odf-decode.ts::decodeOdfOrPatch`: an `ODF1`
// full snapshot registers the base id; an `ODP1` patch whose `b` matches the
// stored base applies; a patch whose `b` does NOT match throws `ODP-BASE` —
// the caller (live.ts) reconnects and the server answers a fresh full
// snapshot, which applies. The committed golden fixtures
// (`crates/odf/tests/fixtures/patch/`) are shared with the Rust suite, so
// this pin and `crates/odf/tests/patch_tdd.rs` hold the same bytes.

import { readFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decodeOdfOrPatch, OdfError } from '../../packages/core/src/client/odf-decode.ts'
import type { Assertion } from './types.ts'

const here = dirname(fileURLToPath(import.meta.url))
const fixtures = join(here, '..', '..', 'crates', 'odf', 'tests', 'fixtures', 'patch')

function b64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('base64')
}

async function load(name: string): Promise<{ base: Uint8Array; odp: Uint8Array; expected: unknown }> {
  const [base, odp, expectedRaw] = await Promise.all([
    readFile(join(fixtures, `patch_${name}.base.odf.bin`)),
    readFile(join(fixtures, `patch_${name}.odp.bin`)),
    readFile(join(fixtures, `patch_${name}.expected.json`), 'utf8'),
  ])
  return { base: new Uint8Array(base), odp: new Uint8Array(odp), expected: JSON.parse(expectedRaw) }
}

const c36: Assertion = {
  id: 'C36',
  name: 'ODF PATCH applies; mismatched base falls back to full (F06)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    // 1. A patch whose base id matches applies and yields the expected value.
    const applyCase = await load('set')
    const v1 = decodeOdfOrPatch(b64(applyCase.base), undefined)
    const patched = decodeOdfOrPatch(b64(applyCase.odp), v1)
    if (JSON.stringify(patched) !== JSON.stringify(applyCase.expected)) {
      throw new Error(
        `the patch applied: got ${JSON.stringify(patched)}, expected ${JSON.stringify(applyCase.expected)}`,
      )
    }

    // 2. A patch whose base id does NOT match throws ODP-BASE (the mismatch
    //    fixture is exactly this case — its `b` is not the base's id).
    const mismatch = await load('base-mismatch')
    const base = decodeOdfOrPatch(b64(mismatch.base), undefined)
    let threw = false
    try {
      decodeOdfOrPatch(b64(mismatch.odp), base)
    } catch (e) {
      threw = e instanceof OdfError
    }
    if (!threw) {
      throw new Error('a mismatched-base patch must throw ODP-BASE (the caller reconnects for a full snapshot)')
    }

    // 3. The fallback: after the mismatch throws, the caller decodes a fresh
    //    full snapshot — `decodeOdfOrPatch` on an ODF1 payload applies without
    //    a base and returns the value (the "falls back to full" half).
    const fresh = decodeOdfOrPatch(b64(mismatch.base), base)
    if (JSON.stringify(fresh) !== JSON.stringify(base)) {
      throw new Error('the full-snapshot fallback applies after a mismatched-base patch threw')
    }
    return { ok: true, detail: 'apply(set) ok; mismatch(base-mismatch) → ODP-BASE; full-snapshot fallback applies' }
  },
}

export default c36
