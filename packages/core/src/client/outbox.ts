// V2-F14 — the client half of the offline-first module: the action
// outbox the offline form submits (and any queued POST) ride while the
// browser is offline. A queued item stores its body and an
// `x-oxide-idem` idempotency key; when connectivity returns, `flush`
// replays every item in order — the server answers the replay from the
// L1 idempotency store (`dispatch::idem::stage_idempotency`), so a
// retried submit inserts exactly once.
//
// The delivery rule (the spec's, kept verbatim): a delivered response
// (2xx) and a PERMANENT rejection (4xx, minus the transient 408/429)
// delete the item; anything else — a network failure, a 5xx, a 408 or a
// 429 — keeps it for the next flush. The server never pins a 5xx (the
// stage stores only 2xx/4xx), so a kept item really does retry into a
// handler that will run again.

type Item = {
  id: string
  url: string
  method: string
  body: string
  headers: Record<string, string>
  tries: number
}

const DB = 'ox-outbox'

async function db(): Promise<IDBDatabase> {
  return new Promise((res, rej) => {
    const r = indexedDB.open(DB, 1)
    r.onupgradeneeded = () => r.result.createObjectStore('q', { keyPath: 'id' })
    r.onsuccess = () => res(r.result)
    r.onerror = () => rej(r.error)
  })
}

// Enqueue one action for the next flush; the item's id IS the
// idempotency key (the server dedupes on it — a retry after a timeout
// that actually delivered cannot insert twice). Returns the id so the
// caller can correlate the eventual replay.
export async function enqueue(it: Omit<Item, 'id' | 'tries'>): Promise<string> {
  const id = crypto.randomUUID()
  const d = await db()
  await new Promise<void>((res, rej) => {
    const t = d.transaction('q', 'readwrite')
    t.objectStore('q').put({ ...it, id, tries: 0, headers: { ...it.headers, 'x-oxide-idem': id } })
    t.oncomplete = () => res()
    t.onerror = () => rej(t.error)
  })
  return id
}

// Replay every queued item (the 'online' listener and the boot's
// connectivity check both call this). `send` is injectable so the tests
// drive the delivery rule without a network.
export async function flush(
  send: (i: Item) => Promise<Response> = (i) => fetch(i.url, { method: i.method, headers: i.headers, body: i.body }),
): Promise<void> {
  const d = await db()
  const items: Item[] = await new Promise((res) => {
    const q = d.transaction('q').objectStore('q').getAll()
    q.onsuccess = () => res(q.result as Item[])
  })
  for (const it of items) {
    try {
      const r = await send(it)
      if (r.ok || (r.status >= 400 && r.status < 500 && r.status !== 408 && r.status !== 429)) await del(d, it.id)
      else throw new Error('retry')
    } catch {
      /* keep for the next flush */
    }
  }
}

async function del(d: IDBDatabase, id: string) {
  await new Promise<void>((res) => {
    const t = d.transaction('q', 'readwrite')
    t.objectStore('q').delete(id)
    t.oncomplete = () => res()
  })
}

// The flush trigger: connectivity returning replays the queue. Guarded
// so the module imports cleanly outside a browser (the node tests).
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => void flush())
}
