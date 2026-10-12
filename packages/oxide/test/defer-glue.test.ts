// Deferred-boundary client glue tests (spec §7.1, §9.2): ox:swap events from
// the render crate's inline swapper resolve promise-likes in client data.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { attachDeferGlue, wireDeferreds } from '../src/client/defer-glue.ts'

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
  assert.equal(typeof (data.a as Promise<unknown>).then, 'function', 'placeholder became promise-like')
  assert.equal(data.plain, 'x')
  assert.deepEqual(data.list, data.list)

  glue.resolve(1, 42)
  assert.equal(await (data.a as Promise<unknown>), 42)

  doc.dispatch('ox:swap', { detail: { id: 2, value: 'streamed' } })
  assert.equal(await (data.list[0] as Promise<unknown>), 'streamed', 'ox:swap event resolves the slot')
  glue.dispose()
})

test('defer glue: ox:swap without a value resolves undefined; explicit reject rejects', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data = { later: { __deferred: 7 }, bad: { __deferred: 8 } }
  wireDeferreds(data, glue)

  doc.dispatch('ox:swap', { detail: { id: 7 } }) // core swapper sends {id} only
  assert.equal(await (data.later as Promise<unknown>), undefined)

  glue.reject(8, new Error('boom'))
  await assert.rejects(() => (data.bad as Promise<unknown>), /boom/)
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
  await assert.rejects(() => (data.x as Promise<unknown>), /kaboom/)
  assert.deepEqual(swapped, [{ id: 3, error: 'kaboom' }])

  // after dispose, events no longer touch the registry
  glue.dispose()
  const data2 = { y: { __deferred: 3 } }
  wireDeferreds(data2, glue)
  doc.dispatch('ox:swap', { detail: { id: 3, value: 'late' } })
  const settled = await (data2.y as Promise<unknown>).then(
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
  assert.equal(typeof ((data.map as Map<string, unknown>).get('k') as Promise<unknown>).then, 'function')
  glue.resolve(5, 'v5')
  glue.resolve(6, 'v6')
  assert.equal(await ((data.map as Map<string, unknown>).get('k') as Promise<unknown>), 'v5')
  assert.equal(await ([...data.set][0] as Promise<unknown>), 'v6')
  glue.dispose()
})
