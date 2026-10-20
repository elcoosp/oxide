// ODF encoder tests (spec §9.5 / D11, PERF-01). The encoder is the JSON
// subset; byte-identity against the Rust encoder is asserted via a golden
// hex, and a round-trip through the client decoder proves interop.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { encodeOdf, isOdfEncodable } from '../src/odf-encode.ts'
import { decodeOdf } from '../src/client/odf-decode.ts'
import { bytesToBase64, fromBase64 } from '../src/bytes.ts'

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

test('isOdfEncodable accepts the rich ODF types (§9.5)', () => {
  // JSON-subset values.
  assert.ok(isOdfEncodable({ a: [1, 2, { b: 'x' }] }))
  // §9.5 rich types the encoder now covers.
  assert.ok(isOdfEncodable({ d: new Date(0) }))
  assert.ok(isOdfEncodable({ m: new Map([['a', 1]]) }))
  assert.ok(isOdfEncodable({ s: new Set([1, 2]) }))
  assert.ok(isOdfEncodable({ b: 123n }))
  assert.ok(isOdfEncodable({ u: new URL('https://x.test/a') }))
  assert.ok(isOdfEncodable({ r: /a+/gi }))
  assert.ok(isOdfEncodable({ ta: new Uint8Array([1, 2, 3]) }))
})

test('isOdfEncodable rejects an Invalid Date and cycles', () => {
  assert.ok(!isOdfEncodable({ d: new Date(Number.NaN) }))
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

// ---------------------------------------------------------------------------
// §9.5: rich types round-trip (P0-4). Before this, the encoder threw on any
// non-JSON value, so `'use cache'` silently stored JSON and a Date came back
// as a string (or the cache entry was dropped entirely).
// ---------------------------------------------------------------------------

test('Date encodes to the DATE tag and round-trips', () => {
  const d = new Date(1_700_000_000_123)
  const back = decodeOdf(encodeOdf(d))
  assert.ok(back instanceof Date, 'decodes to a Date')
  assert.equal((back as Date).getTime(), d.getTime())
})

test('BigInt encodes to the BIGINT tag and round-trips', () => {
  const v = -12345678901234567890n
  const back = decodeOdf(encodeOdf(v))
  assert.equal(typeof back, 'bigint')
  assert.equal(back, v)
})

test('Map preserves key order and value types', () => {
  const m = new Map<unknown, unknown>([
    ['a', 1],
    [2, new Date(0)],
  ])
  const back = decodeOdf(encodeOdf(m)) as Map<unknown, unknown>
  assert.ok(back instanceof Map)
  assert.equal(back.get('a'), 1)
  assert.ok(back.get(2) instanceof Date)
})

test('Set round-trips, including a nested Date', () => {
  const s = new Set<unknown>([1, 'two', new Date(5000)])
  const back = decodeOdf(encodeOdf(s)) as Set<unknown>
  assert.ok(back instanceof Set)
  assert.equal(back.size, 3)
  const dates = [...back].filter((x) => x instanceof Date)
  assert.equal(dates.length, 1)
  assert.equal((dates[0] as Date).getTime(), 5000)
})

test('URL and RegExp round-trip', () => {
  const u = new URL('https://example.test/path?q=1')
  const backU = decodeOdf(encodeOdf(u))
  assert.ok(backU instanceof URL)
  assert.equal((backU as URL).toString(), u.toString())

  const r = /ab+c/gi
  const backR = decodeOdf(encodeOdf(r))
  assert.ok(backR instanceof RegExp)
  assert.equal((backR as RegExp).source, 'ab+c')
  assert.equal((backR as RegExp).flags, 'gi')
})

test('typed arrays round-trip element-exactly', () => {
  const f = new Float64Array([1.5, -2.25, 3])
  const backF = decodeOdf(encodeOdf(f)) as Float64Array
  assert.ok(backF instanceof Float64Array)
  assert.deepEqual([...backF], [...f])

  const i = new Int32Array([-1, 0, 2147483647])
  const backI = decodeOdf(encodeOdf(i)) as Int32Array
  assert.deepEqual([...backI], [...i])
})

test('a Date nested in an object survives a round-trip (the cache case)', () => {
  const v = { post: { createdAt: new Date(42), title: 'Hi' } }
  const back = decodeOdf(encodeOdf(v)) as { post: { createdAt: unknown; title: string } }
  assert.ok(back.post.createdAt instanceof Date)
  assert.equal((back.post.createdAt as Date).getTime(), 42)
  assert.equal(back.post.title, 'Hi')
})

// ---------------------------------------------------------------------------
// §9.5 byte-parity: the TS encoder must produce the SAME bytes as the Rust
// encoder (`crates/odf`) for every rich type. Round-tripping TS→TS only
// proves internal consistency; these goldens were generated by
// `oxide_odf::encode(&Value::...)` and lock cross-language interop.
// ---------------------------------------------------------------------------

test('parity: Date matches the Rust encoder', () => {
  const want = '4f44463100067b68e5cf8b010000'
  assert.equal(hex(encodeOdf(new Date(1_700_000_000_123))), want)
})

test('parity: BigInt matches the Rust encoder', () => {
  const want = '4f4446310004152d3132333435363738393031323334353637383930'
  assert.equal(hex(encodeOdf(-12345678901234567890n)), want)
})

test('parity: Map matches the Rust encoder', () => {
  const want = '4f444631000b0105016103000000000000f03f'
  assert.equal(hex(encodeOdf(new Map<unknown, unknown>([['a', 1]]))), want)
})

test('parity: Set matches the Rust encoder', () => {
  const want = '4f444631000c0203000000000000f03f030000000000000040'
  assert.equal(hex(encodeOdf(new Set([1, 2]))), want)
})

test('parity: URL matches the Rust encoder', () => {
  const want = '4f44463100071d68747470733a2f2f6578616d706c652e746573742f706174683f713d31'
  assert.equal(hex(encodeOdf(new URL('https://example.test/path?q=1'))), want)
})

test('parity: RegExp matches the Rust encoder', () => {
  const want = '4f44463100080461622b63026769'
  assert.equal(hex(encodeOdf(/ab+c/gi)), want)
})

test('parity: a Float64Array matches the Rust encoder', () => {
  const want = '4f444631000d0818000000000000f83f00000000000002c00000000000000840'
  assert.equal(hex(encodeOdf(new Float64Array([1.5, -2.25, 3]))), want)
})

test('parity: a nested object with a Date matches the Rust encoder (cache case)', () => {
  const want =
    '4f444631000a0104706f73740a0209637265617465644174062a00000000000000057469746c6505024869'
  assert.equal(hex(encodeOdf({ post: { createdAt: new Date(42), title: 'Hi' } })), want)
})

test('P-09: bytesToBase64 round-trips a 2 MiB payload', () => {
  // Linear-time base64 (chunked) must match the per-byte reference exactly.
  const bytes = new Uint8Array(2 * 1024 * 1024)
  for (let i = 0; i < bytes.length; i++) bytes[i] = i & 0xff
  const b64 = bytesToBase64(bytes)
  const back = fromBase64(b64)
  assert.equal(back.length, bytes.length)
  assert.deepEqual(back, bytes)
  // Header of the base64 must match the reference implementation.
  let bin = ''
  // 96 is a multiple of 3, so the base64 prefix aligns with the input prefix.
  for (const b of bytes.subarray(0, 96)) bin += String.fromCharCode(b)
  assert.ok(b64.startsWith(btoa(bin)))
})
