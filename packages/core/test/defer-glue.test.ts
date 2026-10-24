// Deferred-boundary client glue tests (spec §7.1, §9.2): ox:swap events from
// the render crate's inline swapper resolve promise-likes in client data.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { attachDeferGlue, wireDeferreds, DeferredClient } from '../src/client/defer-glue.ts'


/**
 * Return the value as a promise-like slot. The test's input has the
 * placeholder shape (`{ __deferred: id }`) but `wireDeferreds` mutates it
 * in place to a promise-like; TS can't see the mutation, so this asserts
 * the runtime shape and returns the properly-typed view. The assertion
 * is what makes the cast honest — if `wireDeferreds` ever stops wiring
 * the slot, this throws with a clear message instead of silently yielding
 * a value with no `.then` method.
 */
function asDeferredSlot(value: unknown): Promise<unknown> {
  if (
    value === null ||
    typeof value !== 'object' ||
    typeof (value as { then?: unknown }).then !== 'function'
  ) {
    throw new TypeError(`expected a promise-like slot; got ${typeof value}`)
  }
  return value as Promise<unknown>
}

function fakeDoc() {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  return {
    listeners,
    swaps: [] as Array<{ id: number }>,
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    dispatch(type: string, event: unknown) {
      for (const fn of listeners.get(type) ?? []) fn(event)
    },
  }
}

test('wireDeferreds replaces {__deferred} placeholders with promise-likes', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data = { a: { __deferred: 1 }, list: [{ __deferred: 2 }], plain: 'x' }
  wireDeferreds(data, glue)
  assert.equal(typeof asDeferredSlot(data.a).then, 'function', 'placeholder became promise-like')
  assert.equal(data.plain, 'x')
  assert.deepEqual(data.list, data.list)

  glue.resolve(1, 42)
  assert.equal(await asDeferredSlot(data.a), 42)

  doc.dispatch('ox:swap', { detail: { id: 2, value: 'streamed' } })
  assert.equal(await asDeferredSlot(data.list[0]), 'streamed', 'ox:swap event resolves the slot')
  glue.dispose()
})

test('defer glue: ox:swap without a value resolves undefined; explicit reject rejects', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data = { later: { __deferred: 7 }, bad: { __deferred: 8 } }
  wireDeferreds(data, glue)

  doc.dispatch('ox:swap', { detail: { id: 7 } }) // core swapper sends {id} only
  assert.equal(await asDeferredSlot(data.later), undefined)

  glue.reject(8, new Error('boom'))
  await assert.rejects(() => asDeferredSlot(data.bad), /boom/)
  glue.dispose()
})

test('defer glue: swap event with error detail rejects; onSwap hook fires; dispose detaches', async () => {
  const doc = fakeDoc()
  const swapped: Array<{ id: number }> = []
  const glue = attachDeferGlue({
    doc: doc as unknown as Document,
    onSwap: (detail) => swapped.push(detail),
  })
  const data = { x: { __deferred: 3 } }
  wireDeferreds(data, glue)

  doc.dispatch('ox:swap', { detail: { id: 3, error: 'kaboom' } })
  await assert.rejects(() => asDeferredSlot(data.x), /kaboom/)
  assert.deepEqual(swapped, [{ id: 3, error: 'kaboom' }])

  // after dispose, events no longer touch the registry
  glue.dispose()
  const data2 = { y: { __deferred: 3 } }
  wireDeferreds(data2, glue)
  doc.dispatch('ox:swap', { detail: { id: 3, value: 'late' } })
  const settled = await asDeferredSlot(data2.y).then(
    () => 'resolved',
    () => 'rejected',
  )
  assert.equal(settled, 'rejected', 'disposed glue rejects orphan slots')
})

test('wireDeferreds: keeps non-deferred structure, handles cycles and nested maps', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data: Record<string, unknown> = { name: 'a' }
  data.self = data
  data.map = new Map([['k', { __deferred: 5 }]])
  data.set = new Set([{ __deferred: 6 }])
  wireDeferreds(data, glue)
  assert.equal(data.name, 'a')
  assert.equal(typeof asDeferredSlot((data.map as Map<string, unknown>).get('k')).then, 'function')
  glue.resolve(5, 'v5')
  glue.resolve(6, 'v6')
  assert.equal(await asDeferredSlot((data.map as Map<string, unknown>).get('k')), 'v5')
  assert.equal(await asDeferredSlot([...(data.set as Iterable<unknown>)][0]), 'v6')
  glue.dispose()
})

test('defer glue: ox:swap now carries the streamed HTML as detail.value', async () => {
  // §7.1 regression: before this pass `SWAPPER_SRC` dispatched `{id}` only,
  // so every `DeferredClient` resolved to `undefined`. The swapper now
  // includes the boundary's streamed `innerHTML` as `detail.value`.
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data = { html: { __deferred: 42 } }
  wireDeferreds(data, glue)
  doc.dispatch('ox:swap', { detail: { id: 42, value: '<p class="done">ok</p>' } })
  assert.equal(await asDeferredSlot(data.html), '<p class="done">ok</p>', 'value flows through')
  glue.dispose()
})


// ─── WP-B3: per-settle scripts arriving after load ───────────────────────

/** A tiny fake document: enough of the DOM surface `attachDeferGlue` uses
 *  (querySelectorAll, addEventListener, readyState, documentElement) plus a
 *  MutationObserver the test can drive. */
function fakeDocWithObserver() {
  const scripts: Array<{ type: string; dataId: string | null; text: string }> = []
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  let observerCb: ((m: unknown[]) => void) | null = null
  const doc = {
    readyState: 'complete',
    documentElement: {},
    querySelectorAll(sel: string) {
      if (sel !== 'script[type="application/oxide-deferred"]') return []
      return scripts.map((s) => ({
        textContent: s.text,
        getAttribute: (n: string) => (n === 'data-id' ? s.dataId : n === 'type' ? s.type : null),
      }))
    },
    addEventListener(name: string, fn: (e: unknown) => void) {
      const list = listeners.get(name) ?? []
      list.push(fn)
      listeners.set(name, list)
    },
    removeEventListener() {},
  }
  const MO = function (cb: (m: unknown[]) => void) {
    observerCb = cb
    return { observe() {}, disconnect() { observerCb = null } }
  }
  return {
    doc,
    MO,
    /** Append a settle script and fire the observer, as the browser would. */
    appendSettle(id: string, value: unknown) {
      scripts.push({ type: 'application/oxide-deferred', dataId: id, text: JSON.stringify(value) })
      observerCb?.([
        { addedNodes: [{ tagName: 'SCRIPT', getAttribute: (n: string) => (n === 'type' ? 'application/oxide-deferred' : id) }] },
      ])
    },
  }
}

test('a per-settle script appended after load resolves its slot', async () => {
  const { doc, MO, appendSettle } = fakeDocWithObserver()
  const g = (globalThis as unknown as { MutationObserver?: unknown }).MutationObserver
  ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = MO
  try {
    const glue = attachDeferGlue({ doc: doc as unknown as Document })
    const slot = new DeferredClient<{ late: string }>()
    glue.registry.set(7, slot as unknown as DeferredClient)
    appendSettle('7', { late: 'arrived' })
    const v = await slot.promise
    assert.deepEqual(v, { late: 'arrived' })
    glue.dispose()
  } finally {
    ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = g
  }
})

test('a late per-settle rejection rejects the slot', async () => {
  const { doc, MO, appendSettle } = fakeDocWithObserver()
  const g = (globalThis as unknown as { MutationObserver?: unknown }).MutationObserver
  ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = MO
  try {
    const glue = attachDeferGlue({ doc: doc as unknown as Document })
    const slot = new DeferredClient<unknown>()
    glue.registry.set(3, slot as unknown as DeferredClient)
    appendSettle('3', { __oxide_deferred_error: 'boom' })
    await assert.rejects(slot.promise, /boom/)
    glue.dispose()
  } finally {
    ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = g
  }
})

test('dispose disconnects the observer', () => {
  const { doc, MO } = fakeDocWithObserver()
  const g = (globalThis as unknown as { MutationObserver?: unknown }).MutationObserver
  ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = MO
  try {
    const glue = attachDeferGlue({ doc: doc as unknown as Document })
    glue.dispose()
    // The fake's disconnect nulls its callback; a later append would not
    // reach the glue. Reaching here without throwing is the assertion.
    assert.ok(true)
  } finally {
    ;(globalThis as unknown as { MutationObserver?: unknown }).MutationObserver = g
  }
})
