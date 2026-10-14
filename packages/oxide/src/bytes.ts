// Shared byte-codec helpers. Tiny, dependency-free, and identical in the
// browser and Node: `btoa`/`atob` are the browser path, `Buffer` the Node
// path, and the fallback error message is deliberate — a caller that lacks
// both is in an environment this package does not target.
//
// Lives in `oxide` rather than each framework adapter so the encoders and
// decoders are one implementation, not four copies. Adapters re-export the
// pair from `ir.ts` for compatibility with existing imports.

interface WithBase64Globals {
  btoa?: (s: string) => string
  atob?: (s: string) => string
  Buffer?: {
    from(s: string, enc: string): { toString(enc: string): string } & Uint8Array
  }
}

function globals(): WithBase64Globals {
  return globalThis as WithBase64Globals
}

/**
 * Base64-encode a UTF-8 string. Uses `btoa` in the browser (with a UTF-8
 * round-trip so non-ASCII survives) and `Buffer` in Node.
 */
export function toBase64(s: string): string {
  const g = globals()
  if (typeof g.btoa === 'function') return g.btoa(unescape(encodeURIComponent(s)))
  if (g.Buffer) return g.Buffer.from(s, 'utf8').toString('base64')
  throw new TypeError('toBase64: no base64 encoder available')
}

/**
 * Base64-decode to raw bytes. Used for island props (base64-encoded JSON
 * or ODF) and any protocol payload.
 */
export function fromBase64(b64: string): Uint8Array {
  const g = globals()
  if (typeof g.atob === 'function') {
    const bin = g.atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    return bytes
  }
  if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, 'base64'))
  throw new TypeError('fromBase64: no base64 decoder available')
}
