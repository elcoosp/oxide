// §V2-F07 — the client resumable-fetch contract: a response marked with
// `x-oxide-stream: <id>` survives a dropped connection. The first pass
// pumps the body; a mid-stream drop reattaches `GET /__oxide/stream/<id>`
// with `Range: bytes=<n>-` carrying the exact byte count, up to five
// backoff-backed tries; a `410` means the stream expired.
//
// Written FIRST (TDD red): these pins failed against the missing
// `packages/core/src/client/resume.ts` module. The wiring (endpoint
// handlers returning resumable responses) is server/CLI-side — see
// docs/deviations.md.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { resumableFetch } from '../src/client/resume.ts'

/** A fetch-call recorder entry. */
type Call = { url: string; init?: RequestInit }

/** A response whose body streams `chunks` then errors (the mid-stream drop). */
function droppingResponse(chunks: Uint8Array[], id: string): Response {
  let i = 0
  // pull-based: an error() during start() would reset the queue and
  // discard the chunks (WHATWG semantics) — the drop must land after the
  // consumer has read them.
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (i < chunks.length) {
        controller.enqueue(chunks[i])
        i += 1
        return
      }
      controller.error(new Error('connection dropped'))
    },
  })
  return new Response(body, {
    status: 200,
    headers: { 'x-oxide-stream': id },
  })
}

/** A response whose body streams `chunks` then completes normally. */
function okResponse(chunks: Uint8Array[], headers: Record<string, string> = {}): Response {
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(c)
      controller.close()
    },
  })
  return new Response(body, { status: 200, headers })
}

const enc = (s: string): Uint8Array => new TextEncoder().encode(s)

test('a response without the stream header passes through untouched', async () => {
  const calls: Call[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    return okResponse([enc('plain')])
  }) as typeof fetch
  const res = await resumableFetch('https://oxide.test/export', undefined, undefined, {
    fetch: fetchImpl,
  })
  assert.equal(calls.length, 1, 'no reattach: the header is absent')
  assert.equal(await res.text(), 'plain')
})

test('the first pass consumes chunks and calls onChunk without reattaching', async () => {
  const calls: Call[] = []
  const seen: Uint8Array[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    return okResponse([enc('a'), enc('b')], { 'x-oxide-stream': 's1' })
  }) as typeof fetch
  const res = await resumableFetch('/export', undefined, (c) => seen.push(c), {
    fetch: fetchImpl,
  })
  assert.equal(calls.length, 1, 'a complete stream never reattaches')
  const merged = seen.map((c) => new TextDecoder().decode(c)).join('')
  assert.equal(merged, 'ab')
  assert.equal(res.headers.get('x-oxide-stream'), 's1')
})

test('a mid-stream drop reattaches with Range bytes=n- carrying the exact count', async () => {
  const calls: Call[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    if (calls.length === 1) return droppingResponse([enc('hello '), enc('world')], 's7')
    // The reattach serves the missed tail and completes.
    return okResponse([enc('!')], { 'x-oxide-stream': 's7' })
  }) as typeof fetch
  const seen: Uint8Array[] = []
  const res = await resumableFetch('/export', undefined, (c) => seen.push(c), {
    fetch: fetchImpl,
  })
  assert.equal(calls.length, 2, 'one reattach')
  assert.equal(calls[1].url, '/__oxide/stream/s7', 'the reattach URL is the data-plane route')
  assert.equal(
    calls[1].init?.headers && (calls[1].init.headers as Record<string, string>).Range,
    'bytes=11-',
    'the Range carries the exact consumed byte count (11 = "hello world".length)'
  )
  const merged = seen.map((c) => new TextDecoder().decode(c)).join('')
  assert.equal(merged, 'hello world!', 'the concatenation is byte-exact across the drop')
  assert.equal(res.status, 200)
})

test('a 410 reattach throws stream expired', async () => {
  const calls: Call[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    if (calls.length === 1) return droppingResponse([enc('x')], 's1')
    return new Response(null, { status: 410 })
  }) as typeof fetch
  await assert.rejects(
    resumableFetch('/export', undefined, undefined, { fetch: fetchImpl }),
    /stream expired/
  )
  assert.equal(calls.length, 2, 'no further tries after the 410')
})

test('a reattach that fails five times throws stream lost after the backoff ladder', async () => {
  const calls: Call[] = []
  const sleeps: number[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    return droppingResponse([enc('chunk')], 's1')
  }) as typeof fetch
  await assert.rejects(
    resumableFetch('/export', undefined, undefined, {
      fetch: fetchImpl,
      sleep: async (ms) => {
        sleeps.push(ms)
      },
    }),
    /stream lost/
  )
  assert.equal(calls.length, 6, 'the initial fetch plus five reattach tries')
  assert.deepEqual(sleeps, [200, 400, 800, 1600, 3200], 'the spec backoff ladder')
})

test('a non-ok reattach status other than 410 keeps retrying the ladder', async () => {
  const calls: Call[] = []
  const fetchImpl = (async (url: RequestInfo, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    if (calls.length === 1) return droppingResponse([enc('x')], 's1')
    if (calls.length === 2) return new Response(null, { status: 503 })
    return okResponse([enc('tail')], { 'x-oxide-stream': 's1' })
  }) as typeof fetch
  const res = await resumableFetch('/export', undefined, undefined, {
    fetch: fetchImpl,
    sleep: async () => {},
  })
  assert.equal(calls.length, 3, 'the 503 spent one try, the next succeeded')
  assert.equal(res.status, 200)
})
