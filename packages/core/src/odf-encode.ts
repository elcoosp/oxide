// ODF encoder — the JS half of the wire format (spec §9.5 / D11).
//
// The Rust encoder (crates/odf/src/encode.rs) is ground truth. This encoder
// covers the value shapes an island's `props` carries: null/undefined/bool/
// number/string/array/object plus BigInt, Date, Map, Set, URL, RegExp and
// typed arrays (see `writeValue`). Only cycles are left to `isOdfEncodable`,
// which reports `false` so `encodeProps` falls back to JSON bytes instead of
// emitting a partial document the decoder would reject.
//
// Wire layout (see odf-decode.ts for the full table):
//   "ODF1" · varu(preamble=0) · value
//   NULL 0x00 · BOOL 0x02 u8 · NUMBER 0x03 f64le · STRING 0x05 varu+utf8
//   ARRAY 0x09 varu(n) items · OBJECT 0x0A varu(n) (varu(klen) key val)…
//
// Object keys are written byte-sorted (matching the Rust BTreeMap order),
// `__proto__`/`constructor` are rejected (the decoder skips them anyway).

import { bytesToBase64 } from './bytes.ts'

const TAG = {
  NULL: 0x00,
  UNDEFINED: 0x01,
  BOOL: 0x02,
  NUMBER: 0x03,
  BIGINT: 0x04,
  STRING: 0x05,
  DATE: 0x06,
  URL: 0x07,
  REGEXP: 0x08,
  ARRAY: 0x09,
  OBJECT: 0x0a,
  MAP: 0x0b,
  SET: 0x0c,
  TYPED_ARRAY: 0x0d,
  // B-14: the three Rust tags the value encoder previously lacked. `Error`
  // is a plain value here; REF/DEFERRED are produced by the shared-ref
  // machinery (`{ __oxide_ref }` / `{ __oxide_deferred }` markers).
  ERROR: 0x0e,
  REF: 0x0f,
  DEFERRED: 0x10,
} as const

/**
 * Typed-array kind byte → the matching global constructor, in the exact order
 * `crates/odf/src/wire.rs` declares (`TypedArrayKind`): 0 = I8 … 10 = BigUint64.
 */
const TYPED_ARRAY_KINDS: Array<{ ctor: unknown; kind: number }> = [
  { ctor: (globalThis as { Int8Array?: unknown }).Int8Array, kind: 0 },
  { ctor: (globalThis as { Uint8Array?: unknown }).Uint8Array, kind: 1 },
  { ctor: (globalThis as { Uint8ClampedArray?: unknown }).Uint8ClampedArray, kind: 2 },
  { ctor: (globalThis as { Int16Array?: unknown }).Int16Array, kind: 3 },
  { ctor: (globalThis as { Uint16Array?: unknown }).Uint16Array, kind: 4 },
  { ctor: (globalThis as { Int32Array?: unknown }).Int32Array, kind: 5 },
  { ctor: (globalThis as { Uint32Array?: unknown }).Uint32Array, kind: 6 },
  { ctor: (globalThis as { Float32Array?: unknown }).Float32Array, kind: 7 },
  { ctor: (globalThis as { Float64Array?: unknown }).Float64Array, kind: 8 },
  { ctor: (globalThis as { BigInt64Array?: unknown }).BigInt64Array, kind: 9 },
  { ctor: (globalThis as { BigUint64Array?: unknown }).BigUint64Array, kind: 10 },
]

function typedArrayKindOf(v: object): number | undefined {
  for (const { ctor, kind } of TYPED_ARRAY_KINDS) {
    if (typeof ctor === 'function' && v instanceof (ctor as new () => object)) return kind
  }
  return undefined
}

function varu(out: number[], n: number): void {
  n = n >>> 0
  while (n >= 0x80) {
    out.push((n & 0x7f) | 0x80)
    n >>>= 7
  }
  out.push(n)
}

function utf8(s: string): number[] {
  return Array.from(new TextEncoder().encode(s))
}

const UTF8_ENCODER = new TextEncoder()

/**
 * B-14: sort object keys by UTF-8 byte order, matching Rust's
 * `BTreeMap<String, _>`. JS `.sort()` compares UTF-16 code units, which
 * diverges for keys mixing BMP (U+E000–U+FFFF) with surrogate pairs.
 */
function byUtf8(a: string, b: string): number {
  const ab = UTF8_ENCODER.encode(a)
  const bb = UTF8_ENCODER.encode(b)
  const n = Math.min(ab.length, bb.length)
  for (let i = 0; i < n; i++) {
    if (ab[i] !== bb[i]) return ab[i] - bb[i]
  }
  return ab.length - bb.length
}

/** varu(len) + UTF-8 bytes — the STRING body shared by several tags. */
function writeString(out: number[], s: string): void {
  const bytes = utf8(s)
  varu(out, bytes.length)
  out.push(...bytes)
}

function writeValue(out: number[], v: unknown): void {
  if (v === null) {
    out.push(TAG.NULL)
    return
  }
  if (v === undefined) {
    out.push(TAG.UNDEFINED)
    return
  }
  if (typeof v === 'boolean') {
    out.push(TAG.BOOL, v ? 1 : 0)
    return
  }
  if (typeof v === 'number') {
    out.push(TAG.NUMBER)
    const buf = new ArrayBuffer(8)
    new DataView(buf).setFloat64(0, v, true)
    out.push(...new Uint8Array(buf))
    return
  }
  if (typeof v === 'string') {
    out.push(TAG.STRING)
    const bytes = utf8(v)
    varu(out, bytes.length)
    out.push(...bytes)
    return
  }
  if (Array.isArray(v)) {
    out.push(TAG.ARRAY)
    varu(out, v.length)
    for (const item of v) writeValue(out, item)
    return
  }
  if (typeof v === 'bigint') {
    // BIGINT (0x04): varu(len) + decimal-string bytes. Rust validates the same
    // `-?(0|[0-9]+)` shape on encode.
    const dec = v.toString(10)
    out.push(TAG.BIGINT)
    const bytes = utf8(dec)
    varu(out, bytes.length)
    out.push(...bytes)
    return
  }
  if (typeof v === 'object') {
    // ERROR (0x0e): name, message, stack? — mirrors encode.rs' field order.
    if (v instanceof Error) {
      out.push(TAG.ERROR)
      writeString(out, typeof v.name === 'string' ? v.name : 'Error')
      writeString(out, typeof v.message === 'string' ? v.message : '')
      if (typeof v.stack === 'string') {
        out.push(1)
        writeString(out, v.stack)
      } else {
        out.push(0)
      }
      return
    }
    // REF (0x0f) / DEFERRED (0x10): shared-table markers emitted by the
    // reference machinery (see `crates/odf/src/encode.rs` `write_ref`).
    const marker = v as { __oxide_ref?: unknown; __oxide_deferred?: unknown; __oxide_deferred_id?: unknown }
    if (typeof marker.__oxide_ref === 'number') {
      out.push(TAG.REF)
      varu(out, marker.__oxide_ref)
      return
    }
    if (
      marker.__oxide_deferred !== undefined &&
      typeof marker.__oxide_deferred_id === 'number'
    ) {
      out.push(TAG.DEFERRED)
      varu(out, marker.__oxide_deferred_id)
      return
    }
    // DATE (0x06): i64 epoch-ms, little-endian — matches `Value::Date(i64)`.
    if (v instanceof Date) {
      const ms = v.getTime()
      if (!Number.isFinite(ms)) {
        throw new TypeError('odf: cannot encode an Invalid Date')
      }
      out.push(TAG.DATE)
      const buf = new ArrayBuffer(8)
      new DataView(buf).setBigInt64(0, BigInt(Math.trunc(ms)), true)
      out.push(...new Uint8Array(buf))
      return
    }
    // MAP (0x0b): varu(pairCount) + alternating key, value records.
    if (v instanceof Map) {
      out.push(TAG.MAP)
      varu(out, v.size)
      for (const [k, val] of v) {
        writeValue(out, k)
        writeValue(out, val)
      }
      return
    }
    // SET (0x0c): varu(count) + records.
    if (v instanceof Set) {
      out.push(TAG.SET)
      varu(out, v.size)
      for (const item of v) writeValue(out, item)
      return
    }
    // URL (0x07): varu(len) + string form.
    if (typeof URL !== 'undefined' && v instanceof URL) {
      const s = v.toString()
      out.push(TAG.URL)
      const bytes = utf8(s)
      varu(out, bytes.length)
      out.push(...bytes)
      return
    }
    // REGEXP (0x08): varu(srcLen) + src + varu(flagsLen) + flags.
    if (v instanceof RegExp) {
      const source = v.source
      const flags = v.flags
      out.push(TAG.REGEXP)
      const sb = utf8(source)
      varu(out, sb.length)
      out.push(...sb)
      const fb = utf8(flags)
      varu(out, fb.length)
      out.push(...fb)
      return
    }
    // TYPED_ARRAY (0x0d): kind byte + varu(byteLength) + raw little-endian
    // element bytes. The JS typed array already stores in platform order,
    // which is little-endian on every target we run (and matches Rust's
    // `to_le_bytes` for the numeric kinds).
    const taKind = typedArrayKindOf(v)
    if (taKind !== undefined) {
      const view = v as ArrayBufferView
      const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength)
      out.push(TAG.TYPED_ARRAY)
      out.push(taKind)
      varu(out, bytes.length)
      out.push(...bytes)
      return
    }
    // Filter the reserved keys BEFORE counting: the declared count must match
    // the number of pairs actually written, or the decoder desyncs.
    const keys = Object.keys(v as Record<string, unknown>)
      .filter((k) => k !== '__proto__' && k !== 'constructor')
      .sort(byUtf8)
    out.push(TAG.OBJECT)
    varu(out, keys.length)
    for (const k of keys) {
      const kb = utf8(k)
      varu(out, kb.length)
      out.push(...kb)
      writeValue(out, (v as Record<string, unknown>)[k])
    }
    return
  }
  throw new TypeError(`odf: cannot encode ${typeof v}`)
}

/** True when `v` is inside the encoder's JSON subset (no cycles, no exotic types). */
export function isOdfEncodable(v: unknown, depth = 0, seen = new Set<object>()): boolean {
  if (depth > 100) return false
  if (v === null || v === undefined) return true
  const t = typeof v
  if (t === 'boolean' || t === 'number' || t === 'string' || t === 'bigint') return true
  if (t !== 'object') return false
  const o = v as object
  if (seen.has(o)) return false // cycle
  seen.add(o)
  let ok = true
  if (Array.isArray(v)) {
    ok = v.every((x) => isOdfEncodable(x, depth + 1, seen))
  } else if (v instanceof Error) {
    ok = true
  } else if (v instanceof Date) {
    ok = Number.isFinite(v.getTime())
  } else if (v instanceof Map) {
    ok = true
    for (const [k, val] of v) {
      if (!isOdfEncodable(k, depth + 1, seen) || !isOdfEncodable(val, depth + 1, seen)) {
        ok = false
        break
      }
    }
  } else if (v instanceof Set) {
    ok = true
    for (const item of v) {
      if (!isOdfEncodable(item, depth + 1, seen)) {
        ok = false
        break
      }
    }
  } else if (typeof URL !== 'undefined' && v instanceof URL) {
    ok = true
  } else if (v instanceof RegExp) {
    ok = true
  } else if (typedArrayKindOf(o) !== undefined) {
    ok = true
  } else if (Object.getPrototypeOf(o) === Object.prototype || Object.getPrototypeOf(o) === null) {
    ok = Object.values(v as Record<string, unknown>).every((x) =>
      isOdfEncodable(x, depth + 1, seen),
    )
  } else {
    ok = false
  }
  seen.delete(o)
  return ok
}

/** Encode a JSON-subset value to ODF bytes (no preamble). */
export function encodeOdf(value: unknown): Uint8Array {
  const out: number[] = [0x4f, 0x44, 0x46, 0x31] // "ODF1"
  varu(out, 0) // empty preamble
  writeValue(out, value)
  return new Uint8Array(out)
}

/**
 * PERF-01: encode island `props` for `data-ox-props`. Returns base64 of the
 * ODF bytes when `props` is inside the JSON subset; otherwise base64 of
 * `JSON.stringify(props)` — the client's `decodeIslandProps` accepts both
 * (it sniffs the `ODF1` magic, falling back to JSON). So a prop carrying a
 * Date/Map/cycle still hydrates; only the common case pays the ODF path.
 */
export function encodePropsB64(props: unknown): string {
  // P-09: linear-time base64 (chunked) instead of per-byte string concat.
  const toB64 = (bytes: Uint8Array): string => bytesToBase64(bytes)
  if (isOdfEncodable(props)) {
    try {
      return toB64(encodeOdf(props))
    } catch {
      // fall through to JSON
    }
  }
  return toB64(new TextEncoder().encode(JSON.stringify(props)))
}
