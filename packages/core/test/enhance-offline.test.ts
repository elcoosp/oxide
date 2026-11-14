// The offline enhance wire (V2-F14): a form submit that cannot reach the
// network rides the outbox with its `x-oxide-idem` key; the server's
// idempotency stage (`dispatch::idem::stage_idempotency`) answers the
// eventual replay, so the retried submit inserts exactly once — the
// spec's "offline `enhance` enqueues" pin, plus the worker-fan-out
// listener the emitted `sw.js` activate step targets.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { enhance, type EnhanceForm } from '../src/client/enhance.ts'
import { SW_FLUSH_MESSAGE, onSwFlushMessage } from '../src/client/outbox.ts'

function fakeForm(attrs: Record<string, string> = {}) {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const form = {
    attrs: { method: 'post', ...attrs } as Record<string, string>,
    listeners,
    dispatchLog: [] as Array<unknown>,
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    getAttribute(n: string) {
      return n in this.attrs ? this.attrs[n] : null
    },
    hasAttribute(n: string) {
      return n in this.attrs
    },
    dispatchEvent(ev: unknown) {
      this.dispatchLog.push(ev)
      return true
    },
    dispatch(type: string, event: unknown) {
      for (const fn of listeners.get(type) ?? []) fn(event)
    },
  }
  return form as EnhanceForm & {
    listeners: Map<string, Array<(e: unknown) => void>>
    dispatchLog: unknown[]
    dispatch(type: string, event: unknown): void
  }
}

/** Structural outbox fake: records enqueued items, can refuse. */
function fakeQueue(refuse = false) {
  const items: Array<{ url: string; method: string; body: string; headers: Record<string, string> }> = []
  return {
    items,
    enqueue: async (it: { url: string; method: string; body: string; headers: Record<string, string> }) => {
      if (refuse) throw new Error('idb unavailable')
      items.push(it)
      return 'queued-id-1'
    },
  }
}

const urlEncodedForm = (form: EnhanceForm) => {
  const fd = new FormData()
  fd.set('body', 'nice post')
  return fd
}

test('offline probe: the submit skips the doomed fetch and enqueues', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue()
  let fetched = 0
  const errors: unknown[] = []
  enhance(form, {
    fetch: ((): typeof fetch => (url, init) => {
      fetched++
      return Promise.resolve(new Response('{}', { headers: { 'content-type': 'application/json' } }))
    })(),
    getFormData: urlEncodedForm,
    outbox: queue,
    isOnline: () => false,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await Promise.resolve()
  // The async submit handler runs to completion before the assertions.
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(fetched, 0, 'an offline submit does not spend the fetch')
  assert.equal(queue.items.length, 1, 'the action rides the outbox')
  assert.equal(errors.length, 0, 'a queued submit is not an error')
  const ev = form.dispatchLog.find((e) => (e as { type: string }).type === 'ox:form-queued')
  assert.ok(ev, 'the form learns the submit was queued')
  assert.equal((ev as { detail: { id: string } }).detail.id, 'queued-id-1')
})

test('a network failure (TypeError) enqueues the submit', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue()
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.reject(new TypeError('fetch failed: network down')),
    getFormData: urlEncodedForm,
    outbox: queue,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(queue.items.length, 1, 'the unreachable action rides the outbox')
  assert.equal(errors.length, 0)
  assert.ok(
    form.dispatchLog.some((e) => (e as { type: string }).type === 'ox:form-queued'),
    'the form learns the submit was queued',
  )
})

test('the queued item carries the faithful request headers', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue()
  enhance(form, {
    fetch: () => Promise.reject(new TypeError('offline')),
    getFormData: urlEncodedForm,
    outbox: queue,
  })
  // Advertise a build id so the queued replay carries the data-plane
  // header the 409 gate answers on.
  const g = globalThis as { __oxide?: { build?: string } }
  const saved = g.__oxide
  g.__oxide = { build: 'test-build-id' }
  try {
    form.dispatch('submit', { preventDefault() {} })
    await new Promise((r) => setTimeout(r, 0))
  } finally {
    g.__oxide = saved
  }
  const item = queue.items[0]
  assert.equal(item.url, '/?/comment')
  assert.equal(item.method, 'POST')
  assert.equal(item.body, 'body=nice+post', 'the urlencoded body serializes to the replayable string')
  assert.equal(item.headers.accept, 'application/oxide+odf, application/json, text/plain')
  assert.equal(
    item.headers['content-type'],
    'application/x-www-form-urlencoded;charset=UTF-8',
    'the replay is a urlencoded POST, exactly what the native form sends',
  )
  assert.equal(item.headers['x-oxide-build'], 'test-build-id')
  assert.equal(item.headers['x-oxide-idem'], undefined, 'the idempotency key is the enqueue-assigned id')
})

test('a live 5xx failure does not enqueue', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue()
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.resolve(new Response('boom', { status: 500 })),
    getFormData: urlEncodedForm,
    outbox: queue,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(queue.items.length, 0, 'a reached-but-failed action is a live error')
  assert.equal(errors.length, 1)
})

test('a live 4xx failure does not enqueue', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue()
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.resolve(new Response('nope', { status: 422 })),
    getFormData: urlEncodedForm,
    outbox: queue,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(queue.items.length, 0)
  assert.equal(errors.length, 1)
})

test('no outbox configured keeps today’s failure path', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.reject(new TypeError('fetch failed')),
    getFormData: urlEncodedForm,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(errors.length, 1, 'without an outbox the failure surfaces as before')
  assert.equal(
    form.dispatchLog.filter((e) => (e as { type: string }).type === 'ox:form-queued').length,
    0,
  )
})

test('a multipart body cannot ride the string-item outbox', async () => {
  const form = fakeForm({ action: '/?/upload', enctype: 'multipart/form-data' })
  const queue = fakeQueue()
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.reject(new TypeError('offline')),
    getFormData: urlEncodedForm,
    outbox: queue,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(queue.items.length, 0, 'the multipart parts have no string shape to store')
  assert.equal(errors.length, 1, 'the failure surfaces instead of a lossy queue entry')
})

test('an enqueue rejection falls through to onError', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const queue = fakeQueue(true)
  const errors: unknown[] = []
  enhance(form, {
    fetch: () => Promise.reject(new TypeError('offline')),
    getFormData: urlEncodedForm,
    outbox: queue,
    onError: (e) => errors.push(e),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(errors.length, 1, 'the queue is best-effort; a refused enqueue is an error')
})

test('onSwFlushMessage answers the worker fan-out with a flush', async () => {
  let flushes = 0
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const reg = {
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
    },
  }
  assert.equal(SW_FLUSH_MESSAGE, 'OXIDE_OUTBOX_FLUSH', 'the wire constant matches the emitted worker')
  const dispose = onSwFlushMessage(() => {
    flushes++
    return Promise.resolve()
  }, reg)
  for (const fn of listeners.get('message') ?? []) fn({ data: { type: SW_FLUSH_MESSAGE } })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(flushes, 1, 'the activate fan-out triggers the flush')
  for (const fn of listeners.get('message') ?? []) fn({ data: { type: 'unrelated' } })
  assert.equal(flushes, 1, 'only the worker’s message flushes')
  dispose()
  for (const fn of listeners.get('message') ?? []) fn({ data: { type: SW_FLUSH_MESSAGE } })
  assert.equal(flushes, 1, 'the disposer removes the listener')
})

test('onSwFlushMessage without a service worker is a no-op disposer', () => {
  const dispose = onSwFlushMessage(() => Promise.resolve(), null)
  assert.doesNotThrow(() => dispose())
})
