// ODF (Oxide Data Format) decoder — the JS half of spec §9.5 / D11.
//
// The Rust encoder (crates/odf/src/{wire,encode}.rs) is the ground truth; this
// file documents the wire layout it produces (crates/odf/docs/format.md does
// not exist at the time of writing, so this header is the authoritative
// JS-side description — verified byte-exactly against the 16 golden fixtures).
//
// ## Wire layout
//
// A payload is `MAGIC` + record stream:
//
//   "ODF1"                          4 magic/version bytes
//   varu(N)                         preamble (pre-resolved shared) entry count
//   N × value(register=false)       preamble entries — their internal
//                                   containers occupy NO slots
//   value(register=true)            root — every container opened in the root
//                                   section registers a shared slot, numbered
//                                   in pre-order (slot = open order)
//
// `varu` = unsigned LEB128 (7 bits/byte, high bit = continuation, ≤5 bytes).
// All multi-byte scalars are little-endian. Tags:
//
//   0x00 NULL               0x09 ARRAY   varu(count) items…
//   0x01 UNDEFINED          0x0A OBJECT  varu(count) (varu(klen) key val)…
//   0x02 BOOL    u8         0x0B MAP     varu(count) (key val)…
//   0x03 NUMBER  f64 bits   0x0C SET     varu(count) items…
//   0x04 BIGINT  str        0x0D TYPED_ARRAY  kind-u8 varu(byteLen) bytes
//   0x05 STRING  str        0x0E ERROR   str str stackFlag(0|1 str?)
//   0x06 DATE    i64 ms     0x0F REF     varu(slot)
//   0x07 URL     str        0x10 DEFERRED varu(id)
//   0x08 REGEXP  str str    (`str` = varu(byteLen) + UTF-8 bytes)
//
// Object keys are written in byte-sorted order (Rust BTreeMap) and the two
// reserved keys `__proto__` / `constructor` never appear (the Rust encoder
// rejects them); this decoder additionally SKIPS them if encountered, per the
// §17.1 "no prototype keys" hardening rule.
//
// ## Decoded JS shapes
//
// NULL→null, UNDEFINED→undefined, BOOL→boolean, NUMBER→number (bit-exact:
// NaN/-0 preserved), BIGINT→bigint, STRING→string, DATE→Date (epochs outside
// the ECMAScript range ±8.64e15 ms — where Date would become Invalid — decode
// to the raw epoch number instead), URL→URL, REGEXP→RegExp, ARRAY→Array,
// OBJECT→plain object, MAP→Map, SET→Set, TYPED_ARRAY→the matching JS typed
// array (11 kinds), ERROR→an Error subclass instance (stack = undefined when
// the record has none), DEFERRED→`{ __deferred: id }` placeholder (the
// defer-glue turns these into promise-likes), REF→the live shared value
// (identity, so cycles close naturally).
//
// ## Hardening (§17.1)
//
// Size limit 256 MiB, depth limit 512, LEB128 ≤5 bytes, bounded ref table
// (1M slots), reserved object keys skipped, truncation/unknown tags/invalid
// scalars all throw [OdfError] with a stable `code`:
// ODF-SIZE · ODF-MAGIC · ODF-DEPTH · ODF-REF · ODF-TAG · ODF-TRUNC.

/** Maximum payload size (matches Rust `MAX_BYTES`). */
export const MAX_BYTES = 256 * 1024 * 1024
/** Maximum nesting depth (matches Rust `MAX_DEPTH`). */
export const MAX_DEPTH = 512
/** Maximum shared-slot table entries (matches Rust `MAX_REFS`). */
export const MAX_REFS = 1 << 20
/** Largest ECMAScript Date time value (±8.64e15 ms). */
const MAX_DATE_MS = 8.64e15

const MAGIC = 0x4f444631 // "ODF1" as a u32 (big-endian read of the 4 bytes)

/** Error thrown by the decoder; `code` is a stable ODF-NNN diagnostic code. */
export class OdfError extends Error {
  code: string
  constructor(code: string, message: string) {
    super(`ODF decode failed [${code}]: ${message}`)
    this.name = 'OdfError'
    this.code = code
  }
}

const TYPED_CTORS = [
  Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
  Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array,
] as const

// kind byte → element size in bytes (wire.rs TypedArrayKind::element_size)
const TYPED_SIZES = [1, 1, 1, 2, 2, 4, 4, 4, 8, 8, 8]

// DataView accessors per kind byte, in TYPED_CTORS order (little-endian).
const TYPED_GETTERS = [
  'getInt8', 'getUint8', 'getUint8', 'getInt16', 'getUint16',
  'getInt32', 'getUint32', 'getFloat32', 'getFloat64', 'getBigInt64', 'getBigUint64',
] as const

/**
 * Raw (pre-`URL`-normalization) strings of decoded URLs. Rust stores
 * `Value::Url(String)` verbatim; `new URL(s).href` percent-encodes e.g.
 * `?q=≥`, so the to-JSON projection prefers the raw form when available.
 */
const RAW_URL = new WeakMap<URL, string>()

/**
 * Decode ODF bytes into rich JS values (see the module header for the mapping).
 * Throws [OdfError] on any malformed, truncated or over-limit payload.
 */
export function decodeOdf(bytes: Uint8Array): unknown {
  if (bytes.length > MAX_BYTES) {
    throw new OdfError('ODF-SIZE', `payload is ${bytes.length} bytes, limit is ${MAX_BYTES}`)
  }
  if (bytes.length < 4 || readU32BE(bytes, 0) !== MAGIC) {
    throw new OdfError('ODF-MAGIC', 'payload does not start with the b"ODF1" magic + version')
  }
  const d = new Decoder(bytes)
  const n = d.varu('shared table entry count')
  if (n > MAX_REFS) {
    throw new OdfError('ODF-REF', `shared table declares ${n} entries, limit is ${MAX_REFS}`)
  }
  for (let i = 0; i < n; i++) d.shared.push(d.value(1, false))
  const root = d.value(1, true)
  if (d.pos !== bytes.length) {
    throw new OdfError('ODF-TRUNC', `trailing bytes after the root value (${bytes.length - d.pos} left)`)
  }
  return root
}

function readU32BE(b: Uint8Array, at: number): number {
  // DataView is the correct API for reading integers from a byte buffer:
  // alignment-safe, big-endian reads with no per-element narrowing.
  // Callers pass `at` from a bounds-checked source (the ODF header is
  // four bytes at offset zero; the length was verified by the decoder
  // before this ran), so this function is total over its intended use.
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength)
  return dv.getUint32(at, false)
}

class Decoder {
  pos = 4 // past the magic
  /** Shared-slot table: preamble entries first, then root containers in pre-order. */
  shared: unknown[] = []
  private bytes: Uint8Array
  private dv: DataView

  constructor(bytes: Uint8Array) {
    this.bytes = bytes
    this.dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  }

  private need(n: number, what: string): void {
    if (this.pos + n > this.bytes.length) {
      throw new OdfError('ODF-TRUNC', `payload ends prematurely: needed ${n} more byte(s) for ${what} at offset ${this.pos}`)
    }
  }

  u8(what: string): number {
    this.need(1, what)
    // `need` above guarantees at least one byte is available at `pos`;
    // DataView.getUint8 is the same read without the `number | undefined`
    // that `Uint8Array[i]` produces under noUncheckedIndexedAccess.
    const byte = this.dv.getUint8(this.pos)
    this.pos++
    return byte
  }

  /** LEB128 varuint, at most 5 bytes; overlong/overflow → ODF-TAG (as in Rust). */
  varu(what: string): number {
    let result = 0
    let shift = 0
    for (;;) {
      const b = this.u8(`varuint(${what})`)
      if (shift === 28) {
        if (b & 0x80) throw new OdfError('ODF-TAG', `varuint encoding exceeds 5 bytes (${what})`)
        if (b > 0x0f) throw new OdfError('ODF-TAG', `varuint overflows u32 (${what})`)
      }
      result |= (b & 0x7f) << shift
      if (!(b & 0x80)) return result >>> 0
      shift += 7
    }
  }

  string(what: string): string {
    const len = this.varu(`string(${what})`)
    this.need(len, `string(${what})`)
    const slice = this.bytes.subarray(this.pos, this.pos + len)
    this.pos += len
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(slice)
    } catch {
      throw new OdfError('ODF-TAG', `string record is not valid UTF-8 (${what})`)
    }
  }

  /**
   * Read one record. `register=true` (root section) means every container
   * opened here takes the next shared slot, pushed BEFORE its children so
   * back-edges (cycles) resolve to the live, partially-filled object.
   */
  value(depth: number, register: boolean): unknown {
    if (depth > MAX_DEPTH) {
      throw new OdfError('ODF-DEPTH', `value is nested deeper than the ${MAX_DEPTH}-level limit`)
    }
    const tag = this.u8('type tag')
    switch (tag) {
      case 0x00: return null
      case 0x01: return undefined
      case 0x02: {
        const b = this.u8('boolean')
        if (b > 1) throw new OdfError('ODF-TAG', `invalid boolean byte ${b}`)
        return b === 1
      }
      case 0x03: {
        this.need(8, 'number')
        const v = this.dv.getFloat64(this.pos, true)
        this.pos += 8
        return v
      }
      case 0x04: {
        const s = this.string('bigint')
        if (!/^-?(0|[0-9]+)$/.test(s)) {
          throw new OdfError('ODF-TAG', `invalid BigInt decimal string \`${s}\` (expected -?(0|[0-9]+))`)
        }
        return BigInt(s)
      }
      case 0x05: return this.string('string')
      case 0x06: {
        this.need(8, 'date')
        const ms = this.dv.getBigInt64(this.pos, true)
        this.pos += 8
        const n = Number(ms)
        // JS Date cannot hold |ms| > 8.64e15 (would decode to Invalid Date);
        // out-of-range epochs decode to the raw epoch number (documented).
        return n > MAX_DATE_MS || n < -MAX_DATE_MS || !Number.isFinite(n) ? n : new Date(n)
      }
      case 0x07: {
        const s = this.string('url')
        let u: URL
        try {
          u = new URL(s)
        } catch {
          throw new OdfError('ODF-TAG', `invalid URL record \`${s}\``)
        }
        RAW_URL.set(u, s)
        return u
      }
      case 0x08: {
        const source = this.string('regexp source')
        const flags = this.string('regexp flags')
        try {
          return new RegExp(source, flags)
        } catch {
          throw new OdfError('ODF-TAG', `invalid RegExp /${source}/${flags}`)
        }
      }
      case 0x09: return this.sequence(depth, register, () => [])
      case 0x0c: return this.sequence(depth, register, () => new Set())
      case 0x0a: {
        // OBJECT — with register=true the empty object is pushed onto the
        // shared table BEFORE its children so back-edges (cycles) resolve to
        // the live object. Reserved keys are read and skipped (§17.1).
        const obj: Record<string, unknown> = register ? this.openSlot({}) : {}
        const count = this.varu('object count')
        for (let i = 0; i < count; i++) {
          const key = this.string('object key')
          if (key === '__proto__' || key === 'constructor') {
            this.value(depth + 1, register) // read (consume) and drop
            continue
          }
          obj[key] = this.value(depth + 1, register)
        }
        return obj
      }
      case 0x0b: {
        const map = register ? this.openSlot(new Map()) : new Map()
        const count = this.varu('map count')
        for (let i = 0; i < count; i++) {
          const k = this.value(depth + 1, register)
          const v = this.value(depth + 1, register)
          map.set(k, v)
        }
        return map
      }
      case 0x0d: {
        const kind = this.u8('typed-array kind')
        if (kind >= TYPED_CTORS.length) {
          throw new OdfError('ODF-TAG', `unknown typed-array kind byte 0x${kind.toString(16).padStart(2, '0')}`)
        }
        // The three tables are kept length-equal by construction; this
        // check is the type-level assertion TS can't infer from the
        // index-equality alone, and a genuine error if the invariant is
        // ever broken by a future edit.
        const size = TYPED_SIZES[kind]
        const ctor = TYPED_CTORS[kind]
        if (size === undefined || ctor === undefined) {
          throw new OdfError('ODF-TAG', `typed-array kind ${kind} has no size/ctor mapping`)
        }
        const len = this.varu('typed-array length')
        if (len % size !== 0) {
          throw new OdfError('ODF-TAG', `typed array payload length ${len} is not a multiple of the ${size}-byte element size`)
        }
        this.need(len, 'typed-array payload')
        const count = len / size
        // Split by kind: kinds 9 and 10 are BigInt arrays (elements are
        // `bigint`); kinds 0..8 are numeric arrays (elements are `number`).
        // The DataView getters return the same primitives. Writing in one
        // loop over the union of array types cannot be typed without help;
        // two loops over the two distinct element types type cleanly.
        if (kind >= 9) {
          const bigCtor = ctor as BigInt64ArrayConstructor | BigUint64ArrayConstructor
          const out = new bigCtor(count)
          const get = TYPED_GETTERS[kind] as 'getBigInt64' | 'getBigUint64'
          for (let i = 0; i < count; i++) {
            out[i] = this.dv[get](this.pos + i * size, true)
          }
          this.pos += len
          return out
        }
        const out = new (ctor as Exclude<typeof ctor, typeof BigInt64Array | typeof BigUint64Array>)(count)
        const get = TYPED_GETTERS[kind] as Exclude<keyof DataView, 'getBigInt64' | 'getBigUint64' | 'buffer' | 'byteLength' | 'byteOffset'>
        for (let i = 0; i < count; i++) {
          // Element-wise reads via DataView: alignment-safe, little-endian.
          out[i] = (this.dv[get] as (o: number, le?: boolean) => number)(this.pos + i * size, true)
        }
        this.pos += len
        return out
      }
      case 0x0e: {
        const name = this.string('error name')
        const message = this.string('error message')
        const flag = this.u8('error stack flag')
        let stack: string | undefined
        if (flag === 1) stack = this.string('error stack')
        else if (flag !== 0) throw new OdfError('ODF-TAG', `invalid stack flag byte ${flag}`)
        const Ctor = (globalThis as Record<string, unknown>)[name] as ErrorConstructor | undefined
        const e = typeof Ctor === 'function' && Ctor.prototype instanceof Error ? new Ctor(message) : new Error(message)
        if (e.name !== name) e.name = name
        e.stack = stack
        return e
      }
      case 0x0f: {
        const i = this.varu('ref index')
        if (i >= this.shared.length) {
          throw new OdfError('ODF-REF', `Ref(${i}) points outside the ${this.shared.length} started slot(s)`)
        }
        return this.shared[i]
      }
      case 0x10: return { __deferred: this.varu('deferred id') }
      default:
        throw new OdfError('ODF-TAG', `unknown type tag 0x${tag.toString(16).padStart(2, '0')}`)
    }
  }

  private sequence(depth: number, register: boolean, make: () => unknown): unknown {
    const container: unknown = register ? this.openSlot(make()) : make()
    const count = this.varu('container count')
    for (let i = 0; i < count; i++) {
      const v = this.value(depth + 1, register)
      if (container instanceof Set) (container as Set<unknown>).add(v)
      else (container as unknown[]).push(v)
    }
    return container
  }

  /** Register a container on the shared table (pre-order slot) and return it. */
  private openSlot<T>(v: T): T {
    if (this.shared.length >= MAX_REFS) {
      throw new OdfError('ODF-REF', `more than ${MAX_REFS} shared slots in one payload`)
    }
    this.shared.push(v)
    return v
  }
}

// ---- to-JSON projection (test/interop convenience, mirrors Value::to_json) ----

function debugKey(k: unknown): string {
  // Mirrors Rust `format!("{k:?}")` for the Value enum (Value::to_json Map keys).
  if (typeof k === 'string') return k
  if (k === null) return 'Null'
  if (k === undefined) return 'Undefined'
  if (typeof k === 'boolean') return `Bool(${k})`
  if (typeof k === 'number') {
    // Rust Debug prints integral f64 as `2.0`; NaN as `NaN`
    return `Number(${Number.isInteger(k) && !Object.is(k, -0) ? `${k}.0` : String(k)})`
  }
  if (k instanceof Date) return `Date(${k.getTime()})`
  if (typeof k === 'bigint') return `BigInt(${k})`
  return String(k)
}

/**
 * Decode and project to a JSON-serializable shape (mirrors Rust `Value::to_json`):
 * undefined→null, NaN/±Inf→null, Date→epoch ms number, BigInt→decimal string,
 * URL→href, RegExp→source, Map→object with Rust-Debug-formatted keys,
 * Set→array, typed array→byte array, Error→{name, message}, resolved Refs→
 * null on revisit (cycles cut, like Value::Ref.to_json).
 */
export function decodeOdfToJson(bytes: Uint8Array): unknown {
  const seen = new WeakSet<object>()
  const walk = (v: unknown): unknown => {
    if (v === null || v === undefined) return null
    switch (typeof v) {
      case 'number':
      case 'boolean':
      case 'string':
        return v
      case 'bigint':
        return v.toString()
      case 'object': {
        if (seen.has(v as object)) return null // cycle/shared revisit → null (Value::Ref)
        seen.add(v as object)
        if (v instanceof Date) return v.getTime()
        if (v instanceof URL) return RAW_URL.get(v) ?? v.href
        if (v instanceof RegExp) return v.source
        if (v instanceof Error) return { name: v.name, message: v.message }
        if (ArrayBuffer.isView(v)) return Array.from(new Uint8Array(v.buffer, v.byteOffset, v.byteLength))
        if (v instanceof Map) {
          const o: Record<string, unknown> = {}
          for (const [k, val] of v) o[debugKey(k)] = walk(val)
          return o
        }
        if (v instanceof Set) return [...v].map(walk)
        if (Array.isArray(v)) return v.map(walk)
        const o: Record<string, unknown> = {}
        for (const [k, val] of Object.entries(v as Record<string, unknown>)) o[k] = walk(val)
        return o
      }
      default:
        return String(v)
    }
  }
  return walk(decodeOdf(bytes))
}
