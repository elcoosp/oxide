// §7.1 / §9.2 / P1-9: `/__data` carries the loader's resolved `defer()`
// values and the request locale. `normalizeInput` used to drop both, so a
// client navigation's deferred slots hung forever and its locale was blank.
// This asserts the mapping and that the glue settles the slots.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRouter, type RouterDoc, type RouterLink } from '../src/client/router.ts'
import { attachDeferGlue, wireDeferreds } from '../src/client/defer-glue.ts'
import { encodeLite } from './helpers/odf-lite.ts'

function fakeDoc(): RouterDoc & { dispatch(t: string, e: unknown): void } {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  return {
    addEventListener(t: string, f: (e: unknown) => void) {
      const l = listeners.get(t) ?? []
      l.push(f)
      listeners.set(t, l)
    },
    removeEventListener(t: string, f: (e: unknown) => void) {
      listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f))
    },
    querySelectorAll() {
      return []
    },
    dispatch(t: string, e: unknown) {
      for (const f of listeners.get(t) ?? []) f(e)
    },
  } as never
}

const fakeHistory = () => {
  const stack: Array<{ state: unknown; url: string }> = [{ state: null, url: '/' }]
  let idx = 0
  return {
    get state() {
      const cur = stack[idx]
      if (cur === undefined) throw new RangeError('fakeHistory: idx out of range')
      return cur.state
    },
    pushState(state: unknown, _t: string, url: string) { stack.splice(idx + 1); stack.push({ state, url }); idx++ },
    replaceState(state: unknown, _t: string, url: string) { stack[idx] = { state, url } },
  }
}

test('P1-9: normalizeInput keeps deferred + locale from /__data', async () => {
  const doc = fakeDoc()
  const history = fakeHistory()
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const odf = encodeLite({
    route: '/blog/a',
    url: '/blog/a',
    params: { slug: 'a' },
    data: { later: { __deferred: 1 } },
    deferred: { '1': 'resolved!' },
    locale: 'fr',
  })
  const updates: any[] = []
  const router = createRouter({
    adapter: { update: (i: unknown) => updates.push(i) } as never,
    doc,
    history,
    location,
    fetch: (async () => new Response(odf as unknown as BodyInit, { status: 200 })) as unknown as typeof fetch,
    scrollTo: () => {},
    getScroll: () => ({ x: 0, y: 0 }),
  })
  router.attach()
  await router.goto('/blog/a')

  assert.equal(updates.length, 1, 'one update')
  assert.deepEqual(updates[0].deferred, { '1': 'resolved!' }, 'deferred survives normalizeInput')
  assert.equal(updates[0].locale, 'fr', 'locale survives normalizeInput')
})

test('P1-9: glue.resolveAll settles the matching slots', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data: any = { later: { __deferred: 7 } }
  wireDeferreds(data, glue)

  assert.equal((data.later as any).status, 'pending', 'pending before resolveAll')
  glue.resolveAll({ '7': 'the value' })
  assert.equal(await (data.later as Promise<unknown>), 'the value', 'resolveAll settles slot 7')
  glue.dispose()
})

test('P1-9: resolveAll ignores unknown and non-numeric ids', async () => {
  const doc = fakeDoc()
  const glue = attachDeferGlue({ doc: doc as unknown as Document })
  const data: any = { v: { __deferred: 3 } }
  wireDeferreds(data, glue)
  // Must not throw on a garbage map.
  glue.resolveAll({ nope: 1, '999': 'orphan', '3': 'ok' })
  assert.equal(await (data.v as Promise<unknown>), 'ok')
  glue.dispose()
})
