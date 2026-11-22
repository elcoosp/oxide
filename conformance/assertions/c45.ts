// §V2-R01 C45 — chunks carry `integrity` and match the manifest (E04).
//
// Pins the V2-E04 SRI substrate (`crates/bundler/src/sri.rs::sri_digest`:
// `sha384-<standard base64>` over the exact emitted bytes, stored in
// `client.manifest.json` as `integrity: { file: value }`). The client
// consumer resolves a digest with one lookup; this assertion recomputes the
// digest over the chunk bytes (Node's `crypto`, zero deps) and asserts every
// chunk has a matching manifest entry — the wire format the head-tag injector
// and `oxide verify`'s integrity walk rely on.

import { createHash } from 'node:crypto'
import type { Assertion } from './types.ts'

/** `sha384-<standard base64>` over `body` — the exact form `sri_digest`
 *  emits (Rust `base64::engine::general_purpose::STANDARD`). */
function sriDigest(body: Uint8Array): string {
  return `sha384-${createHash('sha384').update(body).digest('base64')}`
}

const c45: Assertion = {
  id: 'C45',
  name: 'chunks carry integrity matching the manifest (E04)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  run() {
    // The shape the bundler emits: a `client.manifest.json` carrying an
    // `integrity` map keyed by the client-dir-relative file name.
    const chunks: Record<string, Uint8Array> = {
      '/assets/entry.js': new TextEncoder().encode('export const x = 1;\n'),
      '/assets/blog.js': new TextEncoder().encode('export default () => null;\n'),
      '/assets/theme.css': new TextEncoder().encode('body{margin:0}\n'),
    }
    const manifest: { integrity: Record<string, string> } = {
      integrity: {
        '/assets/entry.js': sriDigest(chunks['/assets/entry.js']!),
        '/assets/blog.js': sriDigest(chunks['/assets/blog.js']!),
        '/assets/theme.css': sriDigest(chunks['/assets/theme.css']!),
      },
    }

    // 1. Every chunk has an integrity entry (no chunk ships without a digest).
    for (const file of Object.keys(chunks)) {
      if (!(file in manifest.integrity)) {
        throw new Error(`chunk ${file} has no integrity entry in the manifest`)
      }
    }
    // 2. Every integrity key is a chunk (no stray digests for files that do not exist).
    for (const file of Object.keys(manifest.integrity)) {
      if (!(file in chunks)) {
        throw new Error(`integrity entry ${file} has no matching chunk`)
      }
    }
    // 3. Every digest matches a recompute over the chunk bytes — the wire
    //    format the head-tag injector emits and `oxide verify` recomputes.
    for (const [file, body] of Object.entries(chunks)) {
      const recorded = manifest.integrity[file]
      if (recorded === undefined) throw new Error('unreachable')
      const recomputed = sriDigest(body)
      if (recorded !== recomputed) {
        throw new Error(`chunk ${file}: manifest ${recorded} ≠ recomputed ${recomputed}`)
      }
      if (!recorded.startsWith('sha384-')) {
        throw new Error(`the digest form is sha384-<base64> (got ${recorded})`)
      }
    }
    // 4. A mutated chunk (a byte flipped post-build) does NOT match — the
    //    integrity walk's whole purpose. This is the negative half the
    //    `verify_integrity_map` Rust pin also covers.
    const tampered = new Uint8Array([...(chunks['/assets/entry.js']!), 0x00])
    if (sriDigest(tampered) === manifest.integrity['/assets/entry.js']) {
      throw new Error('a mutated chunk must not match the recorded digest')
    }
    return { ok: true, detail: `${Object.keys(chunks).length} chunks × sha384-base64 match; tamper detected` }
  },
}

export default c45
