// §V2-R01 C43 — offline submit yields exactly one server insert (F14).
//
// Pins `packages/core/src/client/outbox.ts`: a queued action carries an
// `x-oxide-idem` idempotency key (the server dedupes on it); `flush` replays
// every queued item in order, a 2xx response deletes the item, and a second
// flush has nothing to replay — exactly one server insert per submit. Node
// has no IndexedDB, so a minimal fake (microtask-driven callbacks) stands in
// for the platform; the delivery rule under test is the outbox's, not IDB's.

import { enqueue, flush } from '../../packages/core/src/client/outbox.ts'
import type { Assertion } from './types.ts'

/** A minimal fake IDB request (the outbox reads `result`, `onsuccess`,
 *  `onerror` on the open + getAll requests). */
interface Req {
  result: unknown
  onupgradeneeded: null | (() => void)
  onsuccess: null | (() => void)
  onerror: null | (() => void)
  error: unknown
}
/** A minimal fake store: `put`/`getAll`/`delete` over a shared `Map`. */
interface Store {
  put(v: { id: string }): Req
  getAll(): Req
  delete(id: string): Req
}
/** A minimal fake transaction: `objectStore` + `oncomplete`/`onerror`. */
interface Tx {
  objectStore(name: string): Store
  oncomplete: null | (() => void)
  onerror: null | (() => void)
  error: unknown
}
interface Database {
  transaction(name: string, mode?: string): Tx
  createObjectStore(name: string, opts?: unknown): Store
}

/** Install a fake `indexedDB` on `globalThis` backed by an in-memory map.
 *  Returns a handle whose `store` is the underlying `Map` (for assertions). */
function installFakeIdb(): { store: Map<string, { id: string; headers: Record<string, string> }>; restore(): void } {
  const stores = new Map<string, Map<string, { id: string; headers: Record<string, string> }>>()
  const g = globalThis as { indexedDB?: unknown }
  const prev = g.indexedDB
  g.indexedDB = {
    open(_name: string, _version?: number): Req {
      const req: Req = { result: null, onupgradeneeded: null, onsuccess: null, onerror: null, error: null }
      queueMicrotask(() => {
        const database: Database = {
          transaction(name: string): Tx {
            const data = stores.get(name) ?? new Map()
            stores.set(name, data)
            const tx: Tx = { objectStore: () => makeStore(data, tx), oncomplete: null, onerror: null, error: null }
            return tx
          },
          createObjectStore(name: string): Store {
            const data = stores.get(name) ?? new Map()
            stores.set(name, data)
            return makeStore(data, null)
          },
        }
        req.result = database
        req.onupgradeneeded?.()
        queueMicrotask(() => req.onsuccess?.())
      })
      return req
    },
  }
  return {
    store: stores.get('q') ?? new Map(),
    restore() {
      g.indexedDB = prev
    },
  }
}

function makeStore(data: Map<string, { id: string; headers: Record<string, string> }>, tx: Tx | null): Store {
  return {
    put(v) {
      data.set(v.id, v)
      if (tx) queueMicrotask(() => tx.oncomplete?.())
      return { result: undefined, onupgradeneeded: null, onsuccess: null, onerror: null, error: null }
    },
    getAll() {
      const req: Req = { result: [...data.values()], onupgradeneeded: null, onsuccess: null, onerror: null, error: null }
      queueMicrotask(() => req.onsuccess?.())
      return req
    },
    delete(id) {
      data.delete(id)
      if (tx) queueMicrotask(() => tx.oncomplete?.())
      return { result: undefined, onupgradeneeded: null, onsuccess: null, onerror: null, error: null }
    },
  }
}

const c43: Assertion = {
  id: 'C43',
  name: 'offline submit yields exactly one server insert (F14)',
  capability: 'offline',
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    const fake = installFakeIdb()
    try {
      // Enqueue one action while "offline". The item's id IS the idempotency
      // key — the server dedupes on `x-oxide-idem`, so a retried submit after
      // a timeout that actually delivered cannot insert twice.
      const id = await enqueue({
        url: 'http://oxide.test/actions',
        method: 'POST',
        body: JSON.stringify({ name: 'Ada' }),
        headers: { 'content-type': 'application/json' },
      })
      if (typeof id !== 'string' || id.length === 0) {
        throw new Error(`enqueue returns the idempotency-key id (got ${id})`)
      }

      // Flush with a send that succeeds (2xx) — the item is delivered exactly
      // once and carries the idempotency key.
      let sends = 0
      let sentIdem = ''
      const send = async (item: { id: string; url: string; method: string; body: string; headers: Record<string, string> }): Promise<Response> => {
        sends++
        sentIdem = item.headers['x-oxide-idem'] ?? ''
        return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { 'content-type': 'application/json' } })
      }
      await flush(send)
      if (sends !== 1) {
        throw new Error(`a delivered submit replays exactly once (sends=${sends}, expected 1)`)
      }
      if (sentIdem !== id) {
        throw new Error(`the replayed item carries x-oxide-idem = the enqueue id (got ${sentIdem}, expected ${id})`)
      }

      // A second flush has nothing to replay — the 2xx deleted the item, so
      // the server sees exactly one insert.
      await flush(send)
      if (sends !== 1) {
        throw new Error(`a second flush never re-delivers (sends=${sends}, expected still 1)`)
      }

      // A PERMANENT rejection (4xx, minus 408/429) also deletes the item: a
      // 400 means "the server saw it and refused" — not a retry.
      const id2 = await enqueue({
        url: 'http://oxide.test/actions',
        method: 'POST',
        body: '{}',
        headers: {},
      })
      let sends2 = 0
      await flush(async () => {
        sends2++
        return new Response('bad request', { status: 400 })
      })
      if (sends2 !== 1) {
        throw new Error(`a 4xx is a permanent rejection — delivered once then deleted (sends=${sends2})`)
      }
      await flush(async () => { sends2++; return new Response('', { status: 200 }) })
      if (sends2 !== 1) {
        throw new Error(`a 4xx-deleted item never retries (sends=${sends2}, expected still 1)`)
      }
      return { ok: true, detail: `2xx → 1 insert + delete; 4xx → 1 insert + delete; idempotency key carried` }
    } finally {
      fake.restore()
    }
  },
}

export default c43
