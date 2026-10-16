// ODF encoder tests (spec §9.5 / D11, PERF-01). The encoder is the JSON
// subset; byte-identity against the Rust encoder is asserted via a golden
// hex, and a round-trip through the client decoder proves interop.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { encodeOdf, isOdfEncodable } from '../src/odf-encode.ts'
import { decodeOdf } from '../src/client/odf-decode.ts'

const hex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, '0')).join('')

test('encodeOdf is byte-identical to the Rust encoder (golden)', () => {
  // Produced by crates/odf's `encode(&Value::from_json(...))`.
  const want = '4f444631000a02016103000000000000f03f01620903020100050178'
  assert.equal(hex(encodeOdf({ a: 1, b: [true, null, 'x'] })), want)
})

test('encodeOdf round-trips through the client decoder', () => {
  const v = { a: 1, b: [true, null, 'x'], nested: { z: [1, 2], y: 'ok' }, f: 1.5, n: null }
  assert.deepEqual(decodeOdf(encodeOdf(v)), v)
})

test('object keys are byte-sorted like the Rust BTreeMap', () => {
  const a = encodeOdf({ z: 1, a: 2, m: 3 })
  const b = encodeOdf({ a: 2, m: 3, z: 1 })
  assert.equal(hex(a), hex(b))
})

test('isOdfEncodable rejects exotic types and cycles', () => {
  assert.ok(isOdfEncodable({ a: [1, 2, { b: 'x' }] }))
  assert.ok(!isOdfEncodable({ d: new Date() }))
  assert.ok(!isOdfEncodable({ m: new Map() }))
  const cyc: Record<string, unknown> = {}
  cyc.self = cyc
  assert.ok(!isOdfEncodable(cyc))
})

test('prototype-polluting keys are rejected by the encoder', () => {
  // Build an object with an OWN `__proto__` key (JSON.parse would set the
  // prototype, not a key). The encoder must skip it.
  const v: Record<string, unknown> = { ok: 2 }
  Object.defineProperty(v, '__proto__', { value: 1, enumerable: true, configurable: true })
  const decoded = decodeOdf(encodeOdf(v)) as Record<string, unknown>
  assert.equal(decoded.ok, 2)
  assert.ok(!Object.prototype.hasOwnProperty.call(decoded, '__proto__'))
})
