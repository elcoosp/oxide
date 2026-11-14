// V2-F14 — the client outbox pins. Written FIRST (TDD red) against the
// missing `packages/core/src/client/outbox.ts` module.
//
// The contract (the spec's verbatim shapes):
// - `enqueue` stores the action with an `x-oxide-idem` header (the id IS
//   the idempotency key — the server dedupes on it) and returns the id.
// - `flush` replays every queued item: a delivered response (2xx) and a
//   PERMANENT rejection (4xx minus the transient 408/429) delete the
//   item; a network failure, a 5xx, a 408 or a 429 KEEP it for the next
//   flush.
// - The server side of that rule is `dispatch::idem::stage_idempotency`
//   (pinned in Rust): the server never pins a 5xx, so a kept item
//   really does retry into a handler that runs again.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { enqueue, flush } from '../src/client/outbox.ts'

// ------------------------------------------------------- the IDB fake

// The minimal in-memory IndexedDB the outbox's usage needs: one object
// store (`q`, keyPath `id`), the three operations the module issues
// (put/getAll/delete) with the event-style completions it awaits.
type Row = { id: string; url: string; method: string; body: string; headers: Record<string, string>; tries: number }

const rows = new Map<string, Row>()

function fakeRequest(result: unknown, mode: 'onsuccess' | 'onerror' = 'onsuccess') {
  const req: Record<string, unknown> = { result }
  queueMicrotask(() => {
    const fn = req[mode]
    if (typeof fn === 'function') fn()
  })
  return req as IDBRequest
}

function fakeObjectStore() {
  return {
    put(value: Row) {
      rows.set(value.id, value)
      return fakeRequest(undefined)
    },
    getAll() {
      return fakeRequest([...rows.values()])
    },
    delete(id: string) {
      rows.delete(id)
      return fakeRequest(undefined)
    },
  }
}

function fakeTransaction(mode: string) {
  const store = fakeObjectStore()
  const t: Record<string, unknown> = { objectStore: () => store }
  // The completion events fire after the request callbacks settle.
  queueMicrotask(() => {
    const fn = t.oncomplete
    if (typeof fn === 'function') fn()
  })
  void mode
  return t as unknown as IDBTransaction
}

// Install the fake before the module under test touches `indexedDB`.
;(globalThis as Record<string, unknown>).indexedDB = {
  open(_name: string, _version: number) {
    const req: Record<string, unknown> = {
      result: {
        transaction: fakeTransaction,
      },
    }
    queueMicrotask(() => {
      const fn = req.onsuccess
      if (typeof fn === 'function') fn()
    })
    return req as IDBOpenDBRequest
  },
}

// ------------------------------------------------------------- pins

test('enqueue stores the action with the idempotency header and returns the id', async () => {
  rows.clear()
  const id = await enqueue({
    url: '/api/subscribe',
    method: 'POST',
    body: '{"email":"a@b.c"}',
    headers: { 'content-type': 'application/json' },
  })
  assert.ok(id, 'an id comes back')
  const stored = rows.get(id)
  assert.ok(stored, 'the item is queued')
  assert.equal(stored.headers['x-oxide-idem'], id, 'the header IS the id')
  assert.equal(stored.tries, 0)
})

test('flush deletes delivered (2xx) items', async () => {
  rows.clear()
  const id = await enqueue({ url: '/a', method: 'POST', body: 'x', headers: {} })
  let calls = 0
  await flush(async () => {
    calls++
    return new Response('', { status: 200 })
  })
  assert.equal(calls, 1)
  assert.ok(!rows.has(id), 'the delivered item is deleted')
})

test('flush deletes permanent rejections (4xx except 408/429)', async () => {
  rows.clear()
  const gone = await enqueue({ url: '/a', method: 'POST', body: 'x', headers: {} })
  const kept = await enqueue({ url: '/b', method: 'POST', body: 'x', headers: {} })
  const transient408 = await enqueue({ url: '/c', method: 'POST', body: 'x', headers: {} })
  const transient429 = await enqueue({ url: '/d', method: 'POST', body: 'x', headers: {} })
  const statuses: Record<string, number> = {
    '/a': 422,
    '/b': 500,
    '/c': 408,
    '/d': 429,
  }
  await flush(async (i) => new Response('', { status: statuses[i.url] }))
  assert.ok(!rows.has(gone), 'the 422 is deleted (permanently rejected)')
  assert.ok(rows.has(kept), 'the 500 is kept for the next flush')
  assert.ok(rows.has(transient408), 'the 408 is kept (transient)')
  assert.ok(rows.has(transient429), 'the 429 is kept (transient)')
})

test('flush keeps items the network failed for', async () => {
  rows.clear()
  const id = await enqueue({ url: '/a', method: 'POST', body: 'x', headers: {} })
  await flush(async () => {
    throw new Error('offline')
  })
  assert.ok(rows.has(id), 'the failed item is kept')
})

test('flush replays every queued item in order with the stored headers', async () => {
  rows.clear()
  await enqueue({ url: '/first', method: 'POST', body: '1', headers: { 'content-type': 'application/json' } })
  await enqueue({ url: '/second', method: 'POST', body: '2', headers: {} })
  const seen: string[] = []
  await flush(async (i) => {
    seen.push(`${i.method} ${i.url} ${i.body} ${Object.keys(i.headers).join(',')}`)
    return new Response('', { status: 204 })
  })
  assert.deepEqual(seen, [
    'POST /first 1 content-type,x-oxide-idem',
    'POST /second 2 x-oxide-idem',
  ])
})
