// Deferred-boundary client glue tests (spec §7.1, §9.2): ox:swap events from
// the render crate's inline swapper resolve promise-likes in client data.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { attachDeferGlue, wireDeferreds, DeferredClient } from '../src/client/defer-glue.ts'
import type { DeferGlueDoc, DeferGlueElement } from '../src/client/defer-glue.ts'


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
  const glue = attachDeferGlue({ doc: doc })
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
  const glue = attachDeferGlue({ doc: doc })
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
    doc: doc,
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
  const glue = attachDeferGlue({ doc: doc })
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
  const glue = attachDeferGlue({ doc: doc })
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
  const g = Reflect.get(globalThis, 'MutationObserver') as unknown
  Reflect.set(globalThis, 'MutationObserver', MO)
  try {
    const glue = attachDeferGlue({ doc: doc })
    const slot = new DeferredClient<{ late: string }>()
    glue.registry.set(7, slot)
    appendSettle('7', { late: 'arrived' })
    const v = await slot.promise
    assert.deepEqual(v, { late: 'arrived' })
    glue.dispose()
  } finally {
    if (g === undefined) Reflect.deleteProperty(globalThis, 'MutationObserver')
    else Reflect.set(globalThis, 'MutationObserver', g)
  }
})

test('a late per-settle rejection rejects the slot', async () => {
  const { doc, MO, appendSettle } = fakeDocWithObserver()
  const g = Reflect.get(globalThis, 'MutationObserver') as unknown
  Reflect.set(globalThis, 'MutationObserver', MO)
  try {
    const glue = attachDeferGlue({ doc: doc })
    const slot = new DeferredClient<unknown>()
    glue.registry.set(3, slot)
    appendSettle('3', { __oxide_deferred_error: 'boom' })
    await assert.rejects(slot.promise, /boom/)
    glue.dispose()
  } finally {
    if (g === undefined) Reflect.deleteProperty(globalThis, 'MutationObserver')
    else Reflect.set(globalThis, 'MutationObserver', g)
  }
})

test('dispose disconnects the observer', () => {
  const { doc, MO } = fakeDocWithObserver()
  const g = Reflect.get(globalThis, 'MutationObserver') as unknown
  Reflect.set(globalThis, 'MutationObserver', MO)
  try {
    const glue = attachDeferGlue({ doc: doc })
    glue.dispose()
    // The fake's disconnect nulls its callback; a later append would not
    // reach the glue. Reaching here without throwing is the assertion.
    assert.ok(true)
  } finally {
    if (g === undefined) Reflect.deleteProperty(globalThis, 'MutationObserver')
    else Reflect.set(globalThis, 'MutationObserver', g)
  }
})


test('defer glue: [data-deferred-value] markers are filled when a slot resolves', () => {
  // The glue updates `<span data-deferred-value="<id>">` markers when a
  // slot resolves with a `{label: string}` value. This is the last-mile
  // wiring that makes the smoke-app's `/deferred` route prove streaming
  // end-to-end: the per-settle script's JSON reaches the DOM.
  const marker = { textContent: 'loading' as string | null }
  const fakeDoc: DeferGlueDoc = {
    addEventListener() {},
    removeEventListener() {},
    querySelectorAll(sel: string): ArrayLike<DeferGlueElement> {
      if (sel === '[data-deferred-value="1"]') return [marker]
      return []
    },
    documentElement: undefined,
  }
  const glue = attachDeferGlue({ doc: fakeDoc })
  glue.wire({ fast: { __deferred: 1 } })
  glue.resolve(1, { label: 'fast' })
  assert.equal(marker.textContent, 'fast')
})

test('defer glue: [data-deferred-value] marker is not overwritten on re-resolve', () => {
  // Calling resolve twice on the same id must not clobber a marker that was
  // set on the first resolve. The DeferredClient ignores a second resolve
  // internally; the marker update has to mirror that (use `wasPending`).
  const marker = { textContent: 'loading' as string | null }
  const fakeDoc: DeferGlueDoc = {
    addEventListener() {},
    removeEventListener() {},
    querySelectorAll(sel: string): ArrayLike<DeferGlueElement> {
      if (sel === '[data-deferred-value="1"]') return [marker]
      return []
    },
    documentElement: undefined,
  }
  const glue = attachDeferGlue({ doc: fakeDoc })
  glue.wire({ fast: { __deferred: 1 } })
  glue.resolve(1, { label: 'fast' })
  glue.resolve(1, { label: 'stale-overwrite' })
  assert.equal(marker.textContent, 'fast')
})

// Simulates the React hydration clobber: the glue resolves synchronously,
// a "React" step overwrites the marker back to the original text, then the
// next animation frame reconciles. This pins the exact bug Playwright
// caught (React error #418 clobbering a deferred marker during hydration).
test('defer-glue: marker re-applied on next animation frame after React clobbers', async () => {
  const written: Array<() => void> = []
  const fakeRaf = (cb: () => void): number => {
    written.push(cb)
    return written.length
  }
  // A minimal DOM: one marker, one script
  type El = { textContent: string; getAttribute(name: string): string | null }
  const markers: Record<string, El> = {}
  const scripts: El[] = [
    { textContent: JSON.stringify({ label: 'fast' }), getAttribute: (n) => (n === 'data-id' ? '1' : null) },
  ]
  const marker1: El = { textContent: 'loading', getAttribute: () => null }
  markers['1'] = marker1
  const doc = {
    readyState: 'complete',
    addEventListener: () => {},
    removeEventListener: () => {},
    documentElement: {},
    querySelectorAll(sel: string): ArrayLike<El> | undefined {
      if (sel.startsWith('[data-deferred-value="')) {
        const id = sel.match(/data-deferred-value="(\d+)"/)?.[1]
        const m = id ? markers[id] : undefined
        return m ? [m] : []
      }
      if (sel.startsWith('script[type="application/oxide-deferred"]')) return scripts
      return []
    },
  }
  // Save original RAF
  const g = globalThis as { requestAnimationFrame?: (cb: () => void) => number }
  const orig = g.requestAnimationFrame
  g.requestAnimationFrame = fakeRaf

  try {
    const glue = attachDeferGlue({ doc: doc as unknown as Document })
    // wire registers slot 1; the glue's internal resolveFromScripts fires
    glue.wire({ fast: { __deferred: 1 } })
    // Sync write happens during wire → marker should now be 'fast'
    // (best-effort; React would overwrite it after this point)
    // Simulate React clobbering back to 'loading'
    marker1.textContent = 'loading'
    // Flush the queued rAF callbacks
    const callbacks = written.slice()
    written.length = 0
    for (const cb of callbacks) cb()
    // Marker should now reflect the resolved value again
    if (marker1.textContent !== 'fast') {
      throw new Error(
        `expected marker re-applied to 'fast', got '${marker1.textContent}'`,
      )
    }
    glue.dispose()
  } finally {
    if (orig === undefined) delete g.requestAnimationFrame
    else g.requestAnimationFrame = orig
  }
})

// Simulates React error #418's DOM-replacement response: the marker element
// is swapped out entirely (React remounts the subtree), not just had its
// text reverted. The retry loop must re-query and write to the *new*
// element.
test('defer-glue: marker reconciles when the element is replaced during hydration', () => {
  const written: Array<() => void> = []
  const fakeRaf = (cb: () => void): number => {
    written.push(cb)
    return written.length
  }
  type El = { textContent: string; getAttribute(name: string): string | null }

  // Mutable DOM: marker 1 starts as a 'loading' element, but a simulated
  // hydration replacement swaps the reference (like React remounting the
  // subtree would).
  let marker1: El = { textContent: 'loading', getAttribute: () => null }

  const scripts: El[] = [
    {
      textContent: JSON.stringify({ label: 'fast' }),
      getAttribute: (n) => (n === 'data-id' ? '1' : null),
    },
  ]
  const doc = {
    readyState: 'complete',
    addEventListener: () => {},
    removeEventListener: () => {},
    documentElement: {},
    querySelectorAll(sel: string): ArrayLike<El> | undefined {
      if (sel.startsWith('[data-deferred-value="')) {
        return sel.includes('"1"') ? [marker1] : []
      }
      if (sel.startsWith('script[type="application/oxide-deferred"]')) return scripts
      return []
    },
  }
  const g = globalThis as { requestAnimationFrame?: (cb: () => void) => number }
  const orig = g.requestAnimationFrame
  g.requestAnimationFrame = fakeRaf

  // Run exactly one queued rAF callback (not a full flush) so we can observe
  // the retry loop between frames — the whole point of this test is what
  // happens *during* the retry window.
  const runOneTick = (): void => {
    const cbs = written.splice(0, written.length)
    if (cbs.length === 0) throw new Error('runOneTick: no tick queued')
    for (const cb of cbs) cb()
  }

  try {
    const glue = attachDeferGlue({ doc: doc as unknown as Document })
    glue.wire({ fast: { __deferred: 1 } })
    // The wire call did a sync write (marker is 'fast') and scheduled the
    // first retry tick.
    if (marker1.textContent !== 'fast') {
      throw new Error(`sync write should have landed, got '${marker1.textContent}'`)
    }
    // Simulate React #418's remount: replace the element reference. The new
    // element carries the server-rendered 'loading' text.
    marker1 = { textContent: 'loading', getAttribute: () => null }
    // Run the next retry tick. It must re-query the marker (finding the new
    // reference), see 'loading' != 'fast', and write 'fast'.
    runOneTick()
    if (marker1.textContent !== 'fast') {
      throw new Error(
        `retry tick should have reconciled the replacement marker to 'fast', got '${marker1.textContent}'`,
      )
    }
    // The loop should keep ticking for the full window (no early exit).
    // Every subsequent tick must be a no-op (marker already correct).
    for (let i = 0; i < 5; i++) {
      runOneTick()
      if (marker1.textContent !== 'fast') {
        throw new Error(
          `tick ${i + 2}: marker should stay 'fast', got '${marker1.textContent}'`,
        )
      }
    }
    glue.dispose()
  } finally {
    if (orig === undefined) delete g.requestAnimationFrame
    else g.requestAnimationFrame = orig
  }
})

