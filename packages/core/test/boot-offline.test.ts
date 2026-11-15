// V2-F14 — the boot's flush-listener adoption pins. Written FIRST (TDD
// red) against the missing wiring: the worker emitter
// (`crates/bundler/src/pwa.rs`) fans an `OXIDE_OUTBOX_FLUSH` message out
// to every client on activate (a build swap is a flush point), and the
// boot module must answer it by replaying the outbox queue — otherwise a
// queued offline action only replays when the platform's `online` event
// fires, which a service-worker-only swap never sends.
//
// The contract (the spec's shapes):
// - importing the boot module registers exactly one service-worker
//   `message` listener (page-lifetime glue, like the outbox module's own
//   `online` listener);
// - a message whose `data.type` is `OXIDE_OUTBOX_FLUSH` replays the real
//   queue (the item's delivery deletes it);
// - any other worker message is ignored.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { enqueue, SW_FLUSH_MESSAGE } from '../src/client/outbox.ts'

// ------------------------------------------------------- the IDB fake
// (the same in-memory shape outbox.test.ts pins against: one object
// store `q`, keyPath `id`, the put/getAll/delete the module issues).

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
  queueMicrotask(() => {
    const fn = t.oncomplete
    if (typeof fn === 'function') fn()
  })
  void mode
  return t as unknown as IDBTransaction
}

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

// ------------------------------------------------ navigator + window fakes
// The boot module registers the flush listener at module scope against
// `navigator.serviceWorker`, so the fakes must be in place BEFORE the
// dynamic `import('../src/client/boot.ts')` below evaluates it.

type SwListener = [type: string, fn: (e: unknown) => void]
const swListeners: SwListener[] = []
const fakeRegistration = {
  addEventListener(type: string, fn: (e: unknown) => void) {
    swListeners.push([type, fn])
  },
  removeEventListener(type: string, fn: (e: unknown) => void) {
    const i = swListeners.findIndex(([t, f]) => t === type && f === fn)
    if (i >= 0) swListeners.splice(i, 1)
  },
}

const realNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator')
Object.defineProperty(globalThis, 'navigator', {
  configurable: true,
  value: { serviceWorker: fakeRegistration },
})
;(globalThis as Record<string, unknown>).window = {}
globalThis.fetch = (async () => new Response('', { status: 200 })) as typeof fetch

function dispatchSwMessage(data: unknown): void {
  for (const [type, fn] of [...swListeners]) {
    if (type === 'message') fn({ data })
  }
}

/** Poll until `cond` holds (the flush chain is fire-and-forget async). */
async function until(cond: () => boolean, what: string): Promise<void> {
  for (let waited = 0; waited < 500; waited += 5) {
    if (cond()) return
    await new Promise((r) => setTimeout(r, 5))
  }
  assert.ok(cond(), what)
}

// The boot module evaluates ONCE per process; import it with the fakes
// already installed (the eager browser-path install + the flush listener
// both run at evaluation).
const boot = await import('../src/client/boot.ts')
void boot

// ------------------------------------------------------------- pins

test('boot registers exactly one service-worker message listener', () => {
  const messages = swListeners.filter(([t]) => t === 'message')
  assert.equal(messages.length, 1, `one message listener, got ${messages.length}`)
})

test('the boot listener answers the flush fan-out by replaying the queue', async () => {
  rows.clear()
  const id = await enqueue({ url: '/queued', method: 'POST', body: 'x=1', headers: {} })
  assert.ok(rows.has(id), 'the action is queued before the fan-out')
  // The worker's activate step fans this out (crates/bundler/src/pwa.rs).
  dispatchSwMessage({ type: SW_FLUSH_MESSAGE })
  await until(() => !rows.has(id), 'the queued action replayed and was deleted')
})

test('the boot listener ignores worker messages that are not the flush signal', async () => {
  rows.clear()
  const id = await enqueue({ url: '/kept', method: 'POST', body: 'x=1', headers: {} })
  dispatchSwMessage({ type: 'SOME_OTHER_MESSAGE' })
  await new Promise((r) => setTimeout(r, 20))
  assert.ok(rows.has(id), 'an unrelated worker message never touches the queue')
})

// No globals restore at the tail: `node --test` runs each test file in
// its own process, so the fakes cannot leak into another file's run.
