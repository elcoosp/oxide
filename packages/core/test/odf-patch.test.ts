// §V2-F06 — the TS half of the ODF patch contract (D30): `decodeOdfOrPatch`
// sniffs `ODP1` versus `ODF1`, applies patches against the previously
// decoded value, and rejects reserved-key paths; the committed golden
// fixtures (crates/odf/tests/fixtures/patch/) are shared with the Rust suite
// (`crates/odf/tests/patch_tdd.rs`), so both implementations are pinned to
// the same bytes — including the truncated SHA-256 base ids.
//
// Written FIRST (red): these pins failed on the missing `decodeOdfOrPatch` /
// `applyPatch` / `sha256Hex16` exports before the odf-decode additions.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decodeOdf, decodeOdfOrPatch, decodeOdfToJson, applyPatch, sha256Hex16, OdfError } from '../src/client/odf-decode.ts'
import { encodeOdf } from '../src/odf-encode.ts'

const HERE = dirname(fileURLToPath(import.meta.url))
const PATCH_FIXTURES = join(HERE, '..', '..', '..', 'crates', 'odf', 'tests', 'fixtures', 'patch')

/** The 12 D30 golden cases; a drift check compares the committed set. */
const ALL_PATCH_FIXTURES = [
  'set',
  'nested-set',
  'delete',
  'append',
  'prepend',
  'middle-splice',
  'truncate',
  'type-change',
  'map-untouched',
  'deep-array-of-objects',
  'no-op',
  'base-mismatch',
] as const

const MISMATCH_CASES = new Set(['base-mismatch'])

function b64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('base64')
}

function loadFixture(name: string): Promise<{ base: Uint8Array; odp: Uint8Array; expectedRaw: string }> {
  return Promise.all([
    readFile(join(PATCH_FIXTURES, `patch_${name}.base.odf.bin`)),
    readFile(join(PATCH_FIXTURES, `patch_${name}.odp.bin`)),
    readFile(join(PATCH_FIXTURES, `patch_${name}.expected.json`), 'utf8'),
  ]).then(([base, odp, expectedRaw]) => ({ base, odp, expectedRaw }))
}

test('odf patch: the committed fixture set matches ALL_PATCH_FIXTURES', async () => {
  const files = await readdir(PATCH_FIXTURES)
  const odps = files.filter((f) => f.endsWith('.odp.bin')).sort()
  assert.deepEqual(
    odps.map((f) => f.replace('patch_', '').replace('.odp.bin', '')),
    [...ALL_PATCH_FIXTURES].sort(),
    'patch fixture file set drifted — update ALL_PATCH_FIXTURES',
  )
})

test('odf patch: every golden fixture applies to its base and matches the expected value', async (t) => {
  for (const name of ALL_PATCH_FIXTURES) {
    await t.test(`patch: ${name}`, async () => {
      const { base, odp, expectedRaw } = await loadFixture(name)
      // The client hashes the payload bytes it last decoded.
      const v1 = decodeOdfOrPatch(b64(base), undefined)
      const expected = JSON.parse(expectedRaw)
      if (MISMATCH_CASES.has(name)) {
        assert.throws(
          () => decodeOdfOrPatch(b64(odp), v1),
          (e: OdfError) => e instanceof OdfError && e.code === 'ODP-BASE',
          'a wrong base id must refuse to apply (the reconnect trigger)',
        )
        assert.equal(expected.error, 'ODP-BASE', 'the mismatch expectation documents the refusal')
        return
      }
      const applied = decodeOdfOrPatch(b64(odp), v1)
      // Project through encodeOdf → decodeOdfToJson (the JSON-safe shape the
      // Rust `Value::to_json` expectation mirrors).
      const mine = JSON.parse(JSON.stringify(decodeOdfToJson(encodeOdf(applied))))
      assert.deepEqual(mine, expected, `${name}: applied value drifted from the expectation`)
    })
  }
})

test('odf patch: the patch base id equals the Rust-side truncated SHA-256 of the base bytes', async () => {
  // Cross-language pin: the `b` field inside every committed ODP1 payload
  // was computed by Rust's sha2; the client must derive the same id from
  // the same bytes.
  for (const name of ALL_PATCH_FIXTURES) {
    if (MISMATCH_CASES.has(name)) continue
    const { base, odp } = await loadFixture(name)
    const inner = decodeOdf(odp.subarray(4)) as { b: string; o: unknown[] }
    assert.equal(inner.b, sha256Hex16(base), `${name}: base ids agree across languages`)
  }
})

test('odf patch: sha256Hex16 matches the standard vectors', () => {
  assert.equal(sha256Hex16(Buffer.from('abc')), 'ba7816bf8f01cfea')
  assert.equal(sha256Hex16(new Uint8Array(0)), 'e3b0c44298fc1c14')
  assert.equal(sha256Hex16(Buffer.from('abc')).length, 16)
})

test('odf patch: a full ODF payload decodes exactly like decodeOdf and registers the base', () => {
  const bytes = encodeOdf({ a: 1, rows: ['x', 'y'] })
  const value = decodeOdfOrPatch(b64(bytes), undefined)
  assert.deepEqual(value, { a: 1, rows: ['x', 'y'] })
})

test('odf patch: a patch chains from the re-encoded applied value', () => {
  // Server-side chain: full1 → patch1 (base = id(full1)) → patch2
  // (base = id(re-encoded applied value)). The client re-encodes with the
  // canonical TS encoder, so ids advance in lockstep; a drift reconnects.
  const full1 = encodeOdf({ count: 1, rows: ['a'] })
  const v1 = decodeOdfOrPatch(b64(full1), undefined)
  const patch1 = makePatch(sha256Hex16(full1), [[0, ['count'], 2]])
  const v2 = decodeOdfOrPatch(b64(patch1), v1)
  assert.deepEqual(v2, { count: 2, rows: ['a'] })
  // The next base id is the SHA-256 of the re-encoded applied value.
  const nextId = sha256Hex16(encodeOdf(v2))
  const patch2 = makePatch(nextId, [[2, ['rows'], 1, 0, ['b']]])
  const v3 = decodeOdfOrPatch(b64(patch2), v2)
  assert.deepEqual(v3, { count: 2, rows: ['a', 'b'] })
})

test('odf patch: a patch with an unregistered or stale base throws ODP-BASE', () => {
  const full1 = encodeOdf({ a: 1 })
  const v1 = decodeOdfOrPatch(b64(full1), undefined)
  const stale = makePatch('deadbeefdeadbeef', [[0, ['a'], 2]])
  assert.throws(
    () => decodeOdfOrPatch(b64(stale), v1),
    (e: OdfError) => e.code === 'ODP-BASE',
  )
  // No base state at all (first event is a patch): refuse, never guess.
  assert.throws(
    () => decodeOdfOrPatch(b64(makePatch(sha256Hex16(full1), [[0, ['a'], 2]])), undefined),
    (e: OdfError) => e.code === 'ODP-BASE',
  )
})

test('odf patch: applyPatch rejects prototype-polluting paths (§17.1)', () => {
  const base = { a: 1 }
  assert.throws(
    () => applyPatch(base, [[0, ['a', '__proto__'], 2]]),
    (e: OdfError) => e.code === 'ODP-KEY',
  )
  assert.throws(
    () => applyPatch(base, [[1, ['constructor']]]),
    (e: OdfError) => e.code === 'ODP-KEY',
  )
  // The base is untouched after the refusals.
  assert.deepEqual(base, { a: 1 })
  assert.equal(({} as Record<string, unknown>).polluted, undefined)
})

test('odf patch: applyPatch mirrors the Rust apply semantics', () => {
  // Set creates a missing leaf but not missing intermediates.
  assert.deepEqual(applyPatch({ a: { b: 1 } }, [[0, ['a', 'new'], 7]]), { a: { b: 1, new: 7 } })
  assert.throws(
    () => applyPatch({ a: 1 }, [[0, ['nope', 'leaf'], 1]]),
    (e: OdfError) => e.code === 'ODP-PATH',
  )
  // Del removes object keys only.
  assert.deepEqual(applyPatch({ a: 1, b: 2 }, [[1, ['a']]]), { b: 2 })
  assert.throws(() => applyPatch([1, 2], [[1, [0]]]), (e: OdfError) => e.code === 'ODP-DEL')
  // Splice checks the target and bounds.
  assert.deepEqual(
    applyPatch({ rows: [1, 2, 3] }, [[2, ['rows'], 1, 1, [9, 8]]]),
    { rows: [1, 9, 8, 3] },
  )
  assert.throws(
    () => applyPatch({ rows: 1 }, [[2, ['rows'], 0, 0, []]]),
    (e: OdfError) => e.code === 'ODP-SPLICE',
  )
  assert.throws(
    () => applyPatch({ rows: [1] }, [[2, ['rows'], 2, 5, []]]),
    (e: OdfError) => e.code === 'ODP-SPLICE',
  )
  // Paths must match the value shape: a malformed segment is a parse-time
  // ODP-SHAPE (same code as the Rust path parser); walking a scalar with a
  // valid segment is ODP-PATH.
  assert.throws(
    () => applyPatch({ a: 1 }, [[0, ['a', [0]], 1]]),
    (e: OdfError) => e.code === 'ODP-SHAPE',
  )
  assert.throws(
    () => applyPatch([1], [[0, ['len'], 1]]),
    (e: OdfError) => e.code === 'ODP-PATH',
  )
})

/** Build ODP1 bytes from a base id and wire ops (the server's envelope). */
function makePatch(baseId: string, ops: unknown[]): Uint8Array {
  const inner = encodeOdf({ b: baseId, o: ops })
  const out = new Uint8Array(4 + inner.length)
  out.set(Buffer.from('ODP1', 'ascii'), 0)
  out.set(inner, 4)
  return out
}
