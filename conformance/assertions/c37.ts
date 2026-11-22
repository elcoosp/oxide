// §V2-R01 C37 — resumable stream reattach yields byte-identical total (F07).
//
// Pins `packages/core/src/client/resume.ts::resumableFetch`: a response
// marked `x-oxide-stream: <id>` that errors mid-stream is reattached at the
// byte offset the consumer already saw — `onChunk` receives each chunk
// exactly once, in order, across the drop. The reattach fetch carries
// `Range: bytes=<n>-`; a `410` means the stream expired (the caller fetches
// fresh). Injectable `fetch` + `sleep` keep this V8-free.

import { resumableFetch } from '../../packages/core/src/client/resume.ts'
import type { Assertion } from './types.ts'

const enc = (s: string): Uint8Array => new TextEncoder().encode(s)

/** A readable body that enqueues `chunks` one per pull and errors after the
 *  `errorAfter`-th pull (−1 → enqueue all and close — the well-formed tail).
 *  Pull-based (not start-based) so an error after enqueuing c1/c2 does not
 *  discard the already-buffered chunks: the reader reads c1, c2, then the
 *  next `read()` rejects. */
function body(chunks: Uint8Array[], errorAfter: number): ReadableStream<Uint8Array> {
  let i = 0
  return new ReadableStream({
    pull(controller) {
      if (errorAfter >= 0 && i === errorAfter) {
        controller.error(new Error('stream dropped mid-flight'))
        return
      }
      if (i >= chunks.length) {
        controller.close()
        return
      }
      controller.enqueue(chunks[i]!)
      i++
    },
  })
}

const c37: Assertion = {
  id: 'C37',
  name: 'resumable stream reattach yields byte-identical total (F07)',
  capability: 'resume',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    const c1 = enc('hello ')
    const c2 = enc('world ')
    const c3 = enc('!')
    const seen: Uint8Array[] = []
    let rangeHeader = ''
    let calls = 0
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls++
      if (calls === 1) {
        // The original response: streams c1, c2, then DROPS (reader errors).
        return new Response(body([c1, c2, c3], 2), {
          headers: { 'x-oxide-stream': 's1' },
        })
      }
      // The reattach: input is `/__oxide/stream/s1`, Range carries the byte
      // offset the consumer already saw. The server serves the tail (c3).
      const url = typeof input === 'string' ? input : input.toString()
      if (!url.includes('/__oxide/stream/s1')) {
        throw new Error(`the reattach hit /__oxide/stream/<id> (got ${url})`)
      }
      const h = new Headers(init?.headers)
      rangeHeader = h.get('range') ?? ''
      return new Response(body([c3], -1))
    }) as typeof fetch
    const sleep = (_ms: number): Promise<void> => Promise.resolve()

    const onChunk = (chunk: Uint8Array): void => {
      seen.push(chunk)
    }
    const res = await resumableFetch('http://oxide.test/r', {}, onChunk, { fetch: fetchImpl, sleep })

    // onChunk received c1, c2 (from the first pump) then c3 (from the
    // reattach) — each exactly once, in order, across the drop.
    const got = seen.map((c) => new TextDecoder().decode(c)).join('')
    if (got !== 'hello world !') {
      throw new Error(`the reattach delivered every chunk once in order: got ${JSON.stringify(got)}`)
    }
    const totalBytes = seen.reduce((n, c) => n + c.length, 0)
    if (totalBytes !== c1.length + c2.length + c3.length) {
      throw new Error(`byte-identical total: ${totalBytes} vs ${(c1.length + c2.length + c3.length)}`)
    }
    // The reattach carried Range: bytes=<len(c1)+len(c2)>- (the offset).
    if (rangeHeader !== `bytes=${c1.length + c2.length}-`) {
      throw new Error(`the reattach carried Range: bytes=<offset>- (got ${rangeHeader})`)
    }
    if (calls !== 2) {
      throw new Error(`one original fetch + one reattach (calls=${calls}, expected 2)`)
    }
    // The returned Response is the original (status/headers), not the reattach.
    if (res.headers.get('x-oxide-stream') !== 's1') {
      throw new Error('resumableFetch returns the original response, not the reattach')
    }
    return { ok: true, detail: `3 chunks via 2 fetches, ${totalBytes} bytes, Range ${rangeHeader}` }
  },
}

export default c37
