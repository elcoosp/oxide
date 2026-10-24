// ODF decoder conformance (spec §9.5, D11, §17.1 hardening).
//
// Consumes the Rust golden fixtures at ../../crates/odf/tests/fixtures/
// (see conformance/fixtures/README.md): `<name>.odf.bin` bytes are decoded by
// the JS decoder and compared against literal rich JS expectations derived
// from the Rust fixture definitions (crates/odf/src/fixtures.rs) and the
// `.expected.json` files (Value::to_json output, with documented conversions:
// Undefined→null, Date→epoch ms number, BigInt→decimal string, URL→string,
// RegExp→source, Map keys→Rust Debug format, Error→{name,message},
// Ref→resolved identity (null in JSON), Deferred→{__deferred:id}).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { decodeOdf, decodeOdfToJson, OdfError } from '../src/client/odf-decode.ts'

const HERE = dirname(fileURLToPath(import.meta.url))
const FIXTURES = join(HERE, '..', '..', '..', 'crates', 'odf', 'tests', 'fixtures')

const ALL_FIXTURES = [
  'bigint', 'cycle', 'deferred', 'empty-containers', 'errors', 'kitchen-sink',
  'map-mixed-keys', 'nested-arrays', 'number-specials', 'object-keys-sorted',
  'primitives', 'set-mixed', 'shared-structure', 'strings', 'temporal', 'typed-arrays',
]

async function loadFixture(name: string): Promise<{ bytes: Uint8Array; expectedJson: string }> {
  const [bin, json] = await Promise.all([
    readFile(join(FIXTURES, `${name}.odf.bin`)),
    readFile(join(FIXTURES, `${name}.expected.json`), 'utf8'),
  ])
  return { bytes: new Uint8Array(bin), expectedJson: json }
}

// ---- rich deep equality (typed arrays, Date, URL, RegExp, Map, Set, BigInt, Error) ----

function deepEq(a: unknown, b: unknown, seen = new Map<object, unknown>()): boolean {
  if (Object.is(a, b)) return true
  if (typeof a === 'object' && a !== null && typeof b === 'object' && b !== null) {
    if (seen.get(a as object) === b) return true // cycle: pair already under comparison
    seen.set(a as object, b)
  }
  if (a instanceof Date && b instanceof Date) return Object.is(a.getTime(), b.getTime())
  if (a instanceof URL && b instanceof URL) return a.href === b.href
  if (a instanceof RegExp && b instanceof RegExp) return a.source === b.source && a.flags === b.flags
  if (typeof a === 'bigint' && typeof b === 'bigint') return a === b
  if (a instanceof Error && b instanceof Error) {
    return a.name === b.name && a.message === b.message && a.stack === b.stack
  }
  if (ArrayBuffer.isView(a) && ArrayBuffer.isView(b)) {
    // DataView has no .length or index signature; cast the view to its
    // underlying TypedArray shape (ArrayBuffer.isView is true for both, but
    // the test only constructs Uint8Array values).
    const ta = a as Uint8Array
    const tb = b as Uint8Array
    if (ta.constructor !== tb.constructor) return false
    if (ta.length !== tb.length) return false
    for (let i = 0; i < ta.length; i++) if (!Object.is(ta[i], tb[i])) return false
    return true
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false
    return a.every((v, i) => deepEq(v, b[i], seen))
  }
  if (a instanceof Map && b instanceof Map) {
    if (a.size !== b.size) return false
    const be = [...b.entries()]
    return [...a.entries()].every(([k, v], i) => {
      const beI = be[i]
      if (beI === undefined) return false
      return deepEq(k, beI[0], seen) && deepEq(v, beI[1], seen)
    })
  }
  if (a instanceof Set && b instanceof Set) {
    if (a.size !== b.size) return false
    const bv = [...b.values()]
    return [...a.values()].every((v, i) => deepEq(v, bv[i], seen))
  }
  if (typeof a === 'object' && typeof b === 'object' && a !== null && b !== null) {
    if ((a as object).constructor !== (b as object).constructor) return false
    const ka = Object.keys(a as object)
    const kb = Object.keys(b as object)
    if (ka.length !== kb.length) return false
    return ka.every((k) => k in (b as object) && deepEq((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k], seen))
  }
  return false
}

// ---- per-fixture rich expectations (ground truth = fixtures.rs + .expected.json) ----

const BIG = {
  f0: 'value-0', f1: 'value-1', f2: 'value-2', f3: 'value-3',
  f4: 4.5, f5: new Date(1700000000000), f6: [1, false, null], f7: 'value-7',
}

function errLike(ctor: ErrorConstructor, message: string, stack?: string): Error {
  const e = new ctor(message)
  e.stack = stack // absent stack decodes to undefined, matching the Rust record
  return e
}

function expectedValue(name: string): unknown {
  switch (name) {
    case 'primitives':
      return { null: null, undefined: undefined, true: true, false: false, int: 42, float: -273.15 }
    case 'number-specials':
      return [0, -0, NaN, Infinity, -Infinity, 5e-324, 1.7976931348623157e308]
    case 'strings':
      return ['', 'héllo 🌍 日本語 ça va', 'line\nbreak "quoted" \\back\\\ttab', '\u0000\u0001\u001fctrl']
    case 'bigint':
      return [0n, 123456789012345678901234567890123456789012345678901234567890n, -98765432109876543210987654321098765432109876543210n]
    case 'temporal':
      return {
        date0: new Date(0),
        'date-neg': new Date(-62135596800000),
        // i64::MAX ms is outside the ECMAScript time range (±8.64e15): the
        // decoder returns the raw epoch number rather than an Invalid Date.
        'date-max': 9223372036854775807,
        url: new URL('https://oxide.rs/docs?a=1&b=2#frag'),
        regexp: /^a.*b$\d/gimsu,
      }
    case 'empty-containers':
      return [[], {}, new Map(), new Set()]
    case 'nested-arrays':
      return [[1, [2, [3]]], 'x']
    case 'object-keys-sorted':
      return { '0': 4, a: 2, z: 1, 'Ä': 3 } // wire order is byte-sorted (BTreeMap)
    case 'map-mixed-keys':
      return new Map<unknown, unknown>([
        [null, 1], [2, 'two'], ['s', null], [true, [1]], [new Date(7), undefined],
      ])
    case 'set-mixed':
      return new Set([null, undefined, 1, '1', []])
    case 'typed-arrays':
      return {
        i8: Int8Array.of(-1, 0, 127),
        u8: Uint8Array.of(0, 1, 254, 255),
        u8clamped: Uint8ClampedArray.of(0, 128, 255),
        i16: Int16Array.of(-2, -1, 0, 1, 2),
        u16: Uint16Array.of(0xffff, 1),
        i32: Int32Array.of(-2, 0, 2),
        u32: Uint32Array.of(0xffffffff, 1),
        f32: Float32Array.of(1.5, -0.25, Infinity),
        f64: Float64Array.of(1.5, -2.25, NaN, -0),
        bigint64: BigInt64Array.of(-(2n ** 63n), 2n ** 63n - 1n),
        biguint64: BigUint64Array.of(2n ** 64n - 1n, 1n),
      }
    case 'errors':
      return [
        errLike(TypeError, 'boom'),
        errLike(RangeError, 'out of range', 'at f (x.ts:1:1)'),
      ]
    case 'deferred':
      return { a: { __deferred: 1 }, b: { __deferred: 2 } }
    case 'kitchen-sink':
      return {
        scalars: [null, undefined, true, -0, NaN, 42n],
        when: new Date(1234567890123),
        where: new URL('https://oxide.rs/?q=≥'),
        re: new RegExp('(a|b)+', 'gi'),
        err: errLike(EvalError, 'no'),
        later: { __deferred: 9 },
        map: new Map<unknown, unknown>([[new Date(1), 'one'], ['two', 2]]),
        set: new Set([null, 1, '1']),
        u8s: Uint8Array.of(1, 2, 3),
        deep: [{ inner: [[]] }],
      }
    case 'shared-structure':
      return [BIG, BIG] // identity asserted separately in the Ref-resolution test
    case 'cycle': {
      // a = { name: 'a', peer: b }, b = { name: 'b', back: a } — identity cycle
      const root: Record<string, unknown> = { name: 'a', peer: null }
      root.peer = { name: 'b', back: root }
      return root
    }
    default:
      return undefined
  }
}

test('odf: every golden fixture decodes to the expected rich JS value', async (t) => {
  const files = await readdir(FIXTURES)
  const bins = files.filter((f) => f.endsWith('.odf.bin')).sort()
  assert.deepEqual(
    bins.map((f) => f.replace('.odf.bin', '')),
    [...ALL_FIXTURES].sort(),
    'fixture file set drifted — update ALL_FIXTURES',
  )

  for (const name of ALL_FIXTURES) {
    await t.test(`fixture: ${name}`, async () => {
      const { bytes, expectedJson } = await loadFixture(name)
      const value = decodeOdf(bytes)
      assert.ok(deepEq(value, expectedValue(name)), `rich mismatch for ${name}`)
      // cross-check against the Rust Value::to_json output where the two
      // mappings agree (shared-structure's .expected.json was produced from
      // the pre-encode Value with refs already folded, so it differs).
      if (name !== 'shared-structure') {
        const mine = JSON.parse(JSON.stringify(decodeOdfToJson(bytes)))
        assert.deepStrictEqual(mine, JSON.parse(expectedJson), `to-json mismatch for ${name}`)
      }
    })
  }
})

test('odf: ref resolution gives live identity (shared-structure, cycle)', async () => {
  const { bytes: shared } = await loadFixture('shared-structure')
  const arr = decodeOdf(shared) as object[]
  assert.equal(arr.length, 2)
  assert.ok(arr[0] === arr[1], 'second occurrence must resolve to the first (Ref(1))')

  const { bytes: cyc } = await loadFixture('cycle')
  const root = decodeOdf(cyc) as Record<string, unknown>
  const peer = root.peer as Record<string, unknown>
  assert.equal(root.name, 'a')
  assert.equal(peer.name, 'b')
  assert.ok(peer.back === root, 'back-edge must be the root object itself (identity cycle)')
  // the .expected.json renders the back-edge as null (Value::Ref.to_json)
  const asJson = JSON.parse(JSON.stringify(decodeOdfToJson(cyc)))
  assert.deepEqual(asJson, JSON.parse('{"name":"a","peer":{"back":null,"name":"b"}}'))
})

test('odf: object keys arrive in wire (byte-sorted) order', async () => {
  const { bytes } = await loadFixture('object-keys-sorted')
  const obj = decodeOdf(bytes) as Record<string, unknown>
  assert.deepEqual(Object.keys(obj), ['0', 'a', 'z', 'Ä'])
  const { bytes: ks } = await loadFixture('kitchen-sink')
  assert.deepEqual(Object.keys(decodeOdf(ks) as Record<string, unknown>), [
    'deep', 'err', 'later', 'map', 're', 'scalars', 'set', 'u8s', 'when', 'where',
  ])
})

// ---- §17.1 hardening: magic, truncation, tags, depth, reserved keys ----

const ODF1 = [0x4f, 0x44, 0x46, 0x31] // "ODF1"

function rejectsWithCode(bytes: number[] | Uint8Array, code: string) {
  assert.throws(() => decodeOdf(Uint8Array.from(bytes as number[])), (e: unknown) => {
    assert.ok(e instanceof OdfError, `expected OdfError, got ${String(e)}`)
    assert.equal(e.code, code)
    return true
  })
}

test('odf hardening: bad magic rejects with ODF-MAGIC', () => {
  rejectsWithCode([], 'ODF-MAGIC')
  rejectsWithCode([0x4f, 0x44], 'ODF-MAGIC')
  rejectsWithCode([0x4f, 0x44, 0x46, 0x30, 0x00], 'ODF-MAGIC')
})

test('odf hardening: truncated fixture prefix throws ODF-TRUNC', async () => {
  const { bytes } = await loadFixture('primitives')
  rejectsWithCode(bytes.slice(0, 40), 'ODF-TRUNC')
  // trailing garbage after the root value
  rejectsWithCode([...bytes, 0x00], 'ODF-TRUNC')
})

test('odf hardening: malformed records reject with ODF-TAG', () => {
  // unknown type tag
  rejectsWithCode([...ODF1, 0x00, 0x7f], 'ODF-TAG')
  // invalid boolean byte
  rejectsWithCode([...ODF1, 0x00, 0x02, 0x02], 'ODF-TAG')
  // varuint overlong (>5 bytes)
  rejectsWithCode([...ODF1, 0x00, 0x05, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 'ODF-TAG')
})

test('odf hardening: depth limit (512) on a deeply nested payload', () => {
  const bytes: number[] = [...ODF1, 0x00]
  for (let i = 0; i < 600; i++) bytes.push(0x09, 0x01) // 600 nested 1-element arrays
  bytes.push(0x00)
  rejectsWithCode(bytes, 'ODF-DEPTH')
  // 511 containers + the leaf at depth 512 is exactly at the limit (the Rust
  // decoder counts every node, leaves included — see worklog Task 2)
  const ok: number[] = [...ODF1, 0x00]
  for (let i = 0; i < 511; i++) ok.push(0x09, 0x01)
  ok.push(0x00)
  assert.ok(decodeOdf(Uint8Array.from(ok)) !== undefined)
})

test('odf hardening: reserved keys __proto__/constructor are skipped (§17.1)', () => {
  const ascii = (s: string) => [...s].map((c) => c.charCodeAt(0))
  // OBJECT with 1 key "__proto__" → BOOL true
  const protoPayload = [...ODF1, 0x00, 0x0a, 0x01, 0x09, ...ascii('__proto__'), 0x02, 0x01]
  const obj = decodeOdf(Uint8Array.from(protoPayload)) as Record<string, unknown>
  assert.deepEqual(Object.keys(obj), [])
  assert.equal(({} as Record<string, unknown>).constructor, Object) // prototype untouched
  // "constructor" is skipped the same way
  const ctorPayload = [...ODF1, 0x00, 0x0a, 0x01, 0x0b, ...ascii('constructor'), 0x00]
  assert.deepEqual(Object.keys(decodeOdf(Uint8Array.from(ctorPayload)) as Record<string, unknown>), [])
})

test('odf: decodeOdfToJson maps Map keys like Rust Debug formatting', async () => {
  const { bytes } = await loadFixture('map-mixed-keys')
  const json = decodeOdfToJson(bytes) as Record<string, unknown>
  assert.deepEqual(Object.keys(json).sort(), ['Bool(true)', 'Date(7)', 'Null', 'Number(2.0)', 's'])
})
