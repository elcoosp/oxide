// Minimal ODF encoder for tests: same wire format as the Rust encoder
// (crates/odf/src/encode.rs) without structural sharing — duplicate containers
// are simply written inline, which the decoder accepts. Supports the subset
// the runtime tests need: null/undefined/bool/number/bigint/string/Date/
// Array/plain object (keys byte-sorted like the Rust BTreeMap)/Map/Set.

export function encodeLite(value: unknown): Uint8Array {
  const out: number[] = [0x4f, 0x44, 0x46, 0x31, 0x00] // "ODF1" + empty preamble
  write(value)
  return Uint8Array.from(out)

  function write(v: unknown): void {
    if (v === null) return void out.push(0x00)
    if (v === undefined) return void out.push(0x01)
    switch (typeof v) {
      case 'boolean': return void out.push(0x02, v ? 1 : 0)
      case 'number':
        out.push(0x03)
        return pushF64(v)
      case 'bigint':
        out.push(0x04)
        return pushStr(v.toString())
      case 'string':
        out.push(0x05)
        return pushStr(v)
      case 'object':
        break
      default:
        throw new TypeError(`encodeLite: unsupported value of type ${typeof v}`)
    }
    if (v instanceof Date) {
      out.push(0x06)
      return pushI64(BigInt(v.getTime()))
    }
    if (Array.isArray(v)) {
      out.push(0x09)
      pushVaru(v.length)
      for (const item of v) write(item)
      return
    }
    if (v instanceof Map) {
      out.push(0x0b)
      pushVaru(v.size)
      for (const [k, val] of v) {
        write(k)
        write(val)
      }
      return
    }
    if (v instanceof Set) {
      out.push(0x0c)
      pushVaru(v.size)
      for (const item of v) write(item)
      return
    }
    // plain object
    const obj = v as Record<string, unknown>
    const keys = Object.keys(obj).sort()
    out.push(0x0a)
    pushVaru(keys.length)
    for (const k of keys) {
      pushStr(k)
      write(obj[k])
    }
  }

  function pushVaru(n: number): void {
    let v = n >>> 0
    for (;;) {
      const b = v & 0x7f
      v >>>= 7
      if (v === 0) {
        out.push(b)
        return
      }
      out.push(b | 0x80)
    }
  }

  function pushStr(s: string): void {
    const bytes = Buffer.from(s, 'utf8')
    pushVaru(bytes.length)
    for (const b of bytes) out.push(b)
  }

  function pushF64(n: number): void {
    const b = new Uint8Array(8)
    new DataView(b.buffer).setFloat64(0, n, true)
    for (const x of b) out.push(x)
  }

  function pushI64(n: bigint): void {
    const b = new Uint8Array(8)
    new DataView(b.buffer).setBigInt64(0, n, true)
    for (const x of b) out.push(x)
  }
}
