// Islands loader tests (spec §10.3): strategy scheduling (load/idle/visible/
// media), hydrateIsland dispatch into the adapter, props decoding.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { hydrateIslands, hydrateIsland, parseStrategy } from '../src/client/islands.ts'
import type { RouterDoc } from '../src/client/router.ts'
import { encodeLite } from './helpers/odf-lite.ts'

function islandEl(attrs: Record<string, string>) {
  return {
    attrs: { 'data-ox-strategy': 'load', ...attrs },
    hydrated: false,
    getAttribute(n: string) {
      return n in this.attrs ? this.attrs[n] : null
    },
    hasAttribute(n: string) {
      return n in this.attrs
    },
    setAttribute(n: string, v: string) {
      this.attrs[n] = v
    },
    matches(sel: string) {
      return sel === 'ox-island[data-ox-chunk]'
    },
  }
}

function fakeDoc(els: unknown[] = []) {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  return {
    listeners,
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener() {},
    querySelectorAll(sel: string) {
      return els.filter((e) => (e as { matches(sel: string): boolean }).matches(sel))
    },
  } as unknown as RouterDoc
}

function fakeAdapter() {
  const calls: Array<{ el: unknown; mod: unknown; props: unknown }> = []
  return {
    calls,
    hydrateIsland(el: unknown, mod: unknown, props: unknown) {
      calls.push({ el, mod, props })
      return () => {}
    },
  }
}

const b64 = (v: unknown) => Buffer.from(encodeLite(v)).toString('base64')

test('islands: parseStrategy reads all four directive forms (§10.3)', () => {
  assert.deepEqual(parseStrategy('load'), { kind: 'load' })
  assert.deepEqual(parseStrategy('idle'), { kind: 'idle' })
  assert.deepEqual(parseStrategy('visible'), { kind: 'visible' })
  assert.deepEqual(parseStrategy('media(max-width: 600px)'), { kind: 'media', query: 'max-width: 600px' })
  assert.deepEqual(parseStrategy('media (prefers-reduced-motion)'), { kind: 'media', query: 'prefers-reduced-motion' })
})

test("islands: 'load' hydrates immediately, others wait", async () => {
  const els = [islandEl({ 'data-ox-chunk': '/chunks/a.js' }), islandEl({ 'data-ox-chunk': '/chunks/b.js', 'data-ox-strategy': 'idle' })]
  const adapter = fakeAdapter()
  const loads: string[] = []
  hydrateIslands({
    adapter,
    doc: fakeDoc(els),
    loadChunk: async (c: string) => {
      loads.push(c)
      return { default: {} }
    },
    requestIdleCallback: () => 0, // never fires: the idle island must stay pending
  })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1, 'only the load island hydrated')
  assert.equal(adapter.calls[0].props, undefined, 'no props attribute → undefined props')
  assert.equal(loads[0], '/chunks/a.js')
})

test("islands: 'idle' hydrates via requestIdleCallback", async () => {
  const idleCbs: Array<() => void> = []
  const ric = (cb: () => void) => {
    idleCbs.push(cb)
    return 1
  }
  const els = [islandEl({ 'data-ox-chunk': '/chunks/b.js', 'data-ox-strategy': 'idle' })]
  const adapter = fakeAdapter()
  hydrateIslands({ adapter, doc: fakeDoc(els), loadChunk: async () => ({}), requestIdleCallback: ric })
  assert.equal(adapter.calls.length, 0)
  assert.equal(idleCbs.length, 1, 'scheduled on requestIdleCallback')
  idleCbs[0]()
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1)
})

test("islands: 'visible' hydrates through IntersectionObserver, guarded against double-hydration", async () => {
  const cbs: Array<(entries: Array<{ target: unknown; isIntersecting: boolean }>) => void> = []
  const observed: unknown[] = []
  class FakeIO {
    constructor(cb: typeof cbs[number]) {
      cbs.push(cb)
    }
    observe(el: unknown) {
      observed.push(el)
    }
    unobserve(el: unknown) {
      const i = observed.indexOf(el)
      if (i >= 0) observed.splice(i, 1)
    }
  }
  const el = islandEl({ 'data-ox-chunk': '/chunks/v.js', 'data-ox-strategy': 'visible' })
  const adapter = fakeAdapter()
  hydrateIslands({
    adapter,
    doc: fakeDoc([el]),
    loadChunk: async () => ({}),
    IntersectionObserver: FakeIO as unknown as new (cb: ConstructorParameters<typeof FakeIO>[0]) => IntersectionObserver,
  })
  assert.deepEqual(observed, [el], 'island observed for visibility')
  cbs[0]([{ target: el, isIntersecting: true }])
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1)
  cbs[0]([{ target: el, isIntersecting: true }])
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1, 'double hydration guarded')
})

test("islands: 'media(...)' hydrates on match now or on change", async () => {
  const changes: Array<() => void> = []
  let wide = false
  const mq = (q: string) => ({
    get matches() {
      return q.includes('600px') || wide
    },
    addEventListener(_t: string, cb: () => void) {
      changes.push(cb)
    },
    removeEventListener() {},
  })
  const nowEl = islandEl({ 'data-ox-chunk': '/chunks/m1.js', 'data-ox-strategy': 'media(max-width: 600px)' })
  const laterEl = islandEl({ 'data-ox-chunk': '/chunks/m2.js', 'data-ox-strategy': 'media(min-width: 900px)' })
  const adapter = fakeAdapter()
  hydrateIslands({ adapter, doc: fakeDoc([nowEl, laterEl]), loadChunk: async () => ({}), matchMedia: mq as unknown as typeof matchMedia })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1, 'matching media hydrates immediately')
  assert.equal(changes.length, 1, 'non-matching media registered a change listener')
  changes[0]()
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 1, 'change without a match is ignored')
  wide = true // the query starts matching
  changes[0]()
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(adapter.calls.length, 2, 'hydrates when the query starts matching')
})

test('islands: props decoded from base64 ODF (and JSON fallback)', async () => {
  const odfEl = islandEl({ 'data-ox-chunk': '/chunks/p.js', 'data-ox-props': b64({ count: 3 }) })
  const jsonEl = islandEl({ 'data-ox-chunk': '/chunks/q.js', 'data-ox-props': Buffer.from(JSON.stringify({ id: 1 })).toString('base64') })
  const adapter = fakeAdapter()
  hydrateIslands({ adapter, doc: fakeDoc([odfEl, jsonEl]), loadChunk: async () => ({ name: 'chunk' }) })
  await new Promise((r) => setTimeout(r, 0))
  assert.deepEqual(adapter.calls[0].props, { count: 3 })
  assert.deepEqual(adapter.calls[1].props, { id: 1 }, 'JSON-encoded props fall back to JSON.parse')
  assert.deepEqual((adapter.calls[0].mod as { name: string }).name, 'chunk')
})

test('islands: hydrateIsland dispatches directly and is idempotent', async () => {
  const el = islandEl({ 'data-ox-chunk': '/chunks/d.js', 'data-ox-props': b64({ n: 1 }) })
  const adapter = fakeAdapter()
  const disposer = await hydrateIsland(el, { adapter, loadChunk: async () => ({}) })
  assert.equal(typeof disposer, 'function')
  assert.equal(el.attrs['data-ox-hydrated'], '1')
  await hydrateIsland(el, { adapter, loadChunk: async () => ({}) })
  assert.equal(adapter.calls.length, 1, 'second call is a no-op')
  assert.deepEqual(adapter.calls[0].props, { n: 1 })
})
