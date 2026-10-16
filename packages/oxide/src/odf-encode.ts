// ODF encoder — the JS half of the wire format (spec §9.5 / D11).
//
// The Rust encoder (crates/odf/src/encode.rs) is ground truth. This is the
// **JSON-subset** encoder: it covers the values an island's `props` actually
// carries (null/undefined/bool/number/string/array/object). Anything outside
// that subset (Date, Map, Set, BigInt, typed arrays, cycles) is NOT encoded
// here — `encodeProps` falls back to JSON bytes, which `decodeIslandProps`
// accepts. That keeps the fast path ODF (the common case) without a partial
// encoder ever emitting bytes the decoder would reject.
//
// Wire layout (see odf-decode.ts for the full table):
//   "ODF1" · varu(preamble=0) · value
//   NULL 0x00 · BOOL 0x02 u8 · NUMBER 0x03 f64le · STRING 0x05 varu+utf8
//   ARRAY 0x09 varu(n) items · OBJECT 0x0A varu(n) (varu(klen) key val)…
//
// Object keys are written byte-sorted (matching the Rust BTreeMap order),
// `__proto__`/`constructor` are rejected (the decoder skips them anyway).

const TAG = {
  NULL: 0x00,
  UNDEFINED: 0x01,
  BOOL: 0x02,
  NUMBER: 0x03,
  STRING: 0x05,
  ARRAY: 0x09,
  OBJECT: 0x0a,
} as const

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
  if (typeof v === 'object') {
    // Filter the reserved keys BEFORE counting: the declared count must match
    // the number of pairs actually written, or the decoder desyncs.
    const keys = Object.keys(v as Record<string, unknown>)
      .filter((k) => k !== '__proto__' && k !== 'constructor')
      .sort()
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
  if (t === 'boolean' || t === 'number' || t === 'string') return true
  if (t !== 'object') return false
  const o = v as object
  if (seen.has(o)) return false // cycle
  seen.add(o)
  let ok = true
  if (Array.isArray(v)) {
    ok = v.every((x) => isOdfEncodable(x, depth + 1, seen))
  } else if (Object.getPrototypeOf(o) === Object.prototype || Object.getPrototypeOf(o) === null) {
    ok = Object.values(v as Record<string, unknown>).every((x) =>
      isOdfEncodable(x, depth + 1, seen),
    )
  } else {
    ok = false // Date/Map/Set/typed arrays/etc.
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
  const toB64 = (bytes: Uint8Array): string => {
    let bin = ''
    for (const b of bytes) bin += String.fromCharCode(b)
    const g = globalThis as { btoa?: (s: string) => string; Buffer?: { from(s: string, enc: string): { toString(enc: string): string } } }
    if (typeof g.btoa === 'function') return g.btoa(bin)
    if (g.Buffer) return g.Buffer.from(bin, 'binary').toString('base64')
    throw new TypeError('encodePropsB64: no base64 encoder available')
  }
  if (isOdfEncodable(props)) {
    try {
      return toB64(encodeOdf(props))
    } catch {
      // fall through to JSON
    }
  }
  return toB64(new TextEncoder().encode(JSON.stringify(props)))
}
