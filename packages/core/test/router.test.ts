// Core client router tests (spec §8.4, D9): link interception, History API,
// /__data fetching with ODF decode, prefetch budget + cancel, scroll
// restoration, view transitions hook. All DOM behind a hand-rolled fake.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRouter, type RouterDoc, type RouterLink } from '../src/client/router.ts'
import type { ClientInput } from '../src/types.ts'
import { encodeLite } from './helpers/odf-lite.ts'

// ---- fakes ----

function linkEl(href: string, attrs: Record<string, string> = {}): RouterLink & { attrs: Record<string, string>; matches(sel: string): boolean } {
  return {
    attrs: { href, ...attrs },
    getAttribute(n: string) {
      return n in this.attrs ? this.attrs[n] : null
    },
    hasAttribute(n: string) {
      return n in this.attrs
    },
    closest(sel: string) {
      return sel === 'a' ? this : null
    },
    matches(sel: string) {
      return sel === 'a[href]'
    },
  } as RouterLink & { attrs: Record<string, string>; matches(sel: string): boolean }
}

function fakeDoc(els: Array<RouterLink & { matches(sel: string): boolean }> = []) {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const vtCalls: Array<() => void> = []
  const doc = {
    addEventListener(type: string, fn: (e: unknown) => void) {
      const list = listeners.get(type) ?? []
      list.push(fn)
      listeners.set(type, list)
    },
    removeEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    querySelectorAll(sel: string) {
      return els.filter((e) => e.matches(sel))
    },
    startViewTransition(cb: () => void) {
      vtCalls.push(cb)
      cb()
      return { finished: Promise.resolve() }
    },
    dispatch(type: string, event: unknown) {
      for (const fn of listeners.get(type) ?? []) fn(event)
    },
  }
  return { doc, listeners, vtCalls, dispatch: doc.dispatch }
}

class FakeHistory {
  stack: Array<{ state: unknown; url: string }> = [{ state: INITIAL_STATE, url: '/' }]
  idx = 0
  pushState(state: unknown, _title: string, url: string) {
    this.stack.splice(this.idx + 1)
    this.stack.push({ state, url })
    this.idx++
  }
  replaceState(state: unknown, _title: string, url: string) {
    this.stack[this.idx] = { state, url }
  }
  get state() {
    const s = this.stack[this.idx]
    if (s === undefined) throw new RangeError('fakeHistory: idx out of range')
    return s.state
  }
}

type FetchCall = { url: string; signal: AbortSignal | undefined }

function makeFetch(routes: Record<string, unknown>) {
  const calls: FetchCall[] = []
  const fetchImpl = (url: string, init?: { signal?: AbortSignal }): Promise<Response> => {
    calls.push({ url, signal: init?.signal })
    const route = new URL(url, 'http://x.test').searchParams.get('route') ?? ''
    if (route === '/hang') return new Promise<Response>(() => {}) // never resolves
    const payload = routes[route] ?? { route, params: {}, url: `http://x.test${route}`, data: { missing: true } }
    return Promise.resolve(new Response(encodeLite(payload)))
  }
  return { fetchImpl, calls }
}

// ---- harness ----

const ROUTES = {
  '/': { route: '/', params: {}, url: 'http://x.test/', data: { title: 'Home' } },
  '/about': { route: '/about', params: {}, url: 'http://x.test/about', data: { title: 'About' } },
  '/blog/a': { route: '/blog/[slug]', params: { slug: 'a' }, url: 'http://x.test/blog/a', data: { title: 'Post A' } },
}

// a hydrated app starts with an oxide-tagged entry for the initial route
const INITIAL_STATE = { __oxide: 1 as const, input: ROUTES['/'], scroll: { x: 0, y: 0 } }

function setup(overrides: Record<string, unknown> = {}, docEls: Parameters<typeof fakeDoc>[0] = []) {
  const { doc, dispatch, vtCalls } = fakeDoc(docEls)
  const history = new FakeHistory()
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const { fetchImpl, calls } = makeFetch(ROUTES)
  const updates: ClientInput[] = []
  const scrolls: Array<[number, number]> = []
  const adapter = { update: (input: ClientInput) => updates.push(input) }
  const router = createRouter({
    adapter,
    manifest: { routes: {} },
    doc,
    history,
    location,
    fetch: fetchImpl as unknown as typeof fetch,
    scrollTo: (x: number, y: number) => scrolls.push([x, y]),
    getScroll: () => ({ x: 0, y: 42 }),
    ...overrides,
  })
  return { router, doc, dispatch, vtCalls, history, location, calls, updates, scrolls, adapter }
}

const settle = () => new Promise<void>((r) => setTimeout(r, 0))

// ---- tests ----

/** Typed indexed access with a runtime bounds check. Throws a clear error
 * (failing the test) instead of returning `T | undefined` — the honest way
 * to satisfy `noUncheckedIndexedAccess` in tests whose fixtures are known
 * non-empty. No casts, no `any`, no `!`, no `?? default`. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
}

/** Typed array element call with a runtime bounds + presence check. Used at
 * call sites of captured callbacks (e.g. `idleCbs[0]()`) under
 * noUncheckedIndexedAccess. */
function mustCall<A extends unknown[]>(arr: readonly ((...args: A) => unknown)[], i: number): (...args: A) => unknown {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustCall: index ${i} out of bounds (len=${arr.length})`)
  return v
}

test('router: goto() fetches /__data, decodes ODF, updates adapter, pushes history', async () => {
  const { router, calls, updates, history } = setup()
  await router.goto('/about')
  assert.equal(calls.length, 1)
  assert.equal(mustEl(calls, 0).url, '/__data?route=%2Fabout')
  assert.equal(updates.length, 1)
  assert.deepEqual(mustEl(updates, 0).data, { title: 'About' })
  assert.equal(mustEl(updates, 0).route, '/about')
  assert.equal(mustEl(history.stack, history.idx).url, '/about')
  // departure scroll saved on the entry we left; the new entry starts at top
  assert.deepEqual((mustEl(history.stack, 0).state as { scroll: { x: number; y: number } }).scroll, { x: 0, y: 42 })
  assert.deepEqual((history.state as { scroll: { x: number; y: number } }).scroll, { x: 0, y: 0 })
  // cached: a second goto makes no new request
  await router.goto('/about')
  assert.equal(calls.length, 1)
  assert.equal(updates.length, 2)
})

test('router: goto() with replace:true uses replaceState', () => {
  const { router, history } = setup()
  return router.goto('/about', { replace: true }).then(() => {
    assert.equal(history.stack.length, 1, 'no new entry pushed')
    assert.equal(mustEl(history.stack, 0).url, '/about')
  })
})

test('router: intercepts clicks on internal <a>, ignores external + modifier keys', async () => {
  const { router, dispatch, updates } = setup()
  router.attach()
  const internal = linkEl('/about')
  dispatch('click', { target: internal, button: 0, preventDefault() { (internal as unknown as { prevented: boolean }).prevented = true } })
  await settle()
  assert.ok((internal as unknown as { prevented?: boolean }).prevented, 'default prevented')
  assert.equal(updates.at(-1)?.route, '/about')

  // external link: not intercepted
  const external = linkEl('https://other.test/x')
  const before = updates.length
  dispatch('click', { target: external, button: 0, preventDefault() { (external as unknown as { prevented: boolean }).prevented = true } })
  assert.ok(!(external as unknown as { prevented?: boolean }).prevented)
  assert.equal(updates.length, before)

  // modifier-click (open in new tab): not intercepted
  const plain = linkEl('/blog/a')
  dispatch('click', { target: plain, button: 0, metaKey: true, preventDefault() { (plain as unknown as { prevented: boolean }).prevented = true } })
  assert.ok(!(plain as unknown as { prevented?: boolean }).prevented)
})

test('router: popstate restores adapter state + scroll; unknown state falls back to fetch', async () => {
  const { router, dispatch, updates, history, scrolls, calls } = setup()
  router.attach()
  await router.goto('/about')
  updates.length = 0
  dispatch('popstate', { state: mustEl(history.stack, 0).state }) // back to '/'
  assert.equal(updates.length, 1)
  assert.deepEqual(mustEl(updates, 0).data, { title: 'Home' })
  assert.deepEqual(scrolls.at(-1), [0, 42], 'scroll restored from the entry state')
  // popstate without oxide state: fetch the current location instead
  dispatch('popstate', { state: null })
  await settle()
  assert.equal(updates.length, 2)
  assert.equal(calls.length, 2)
})

test('router: prefetch on mouseover caches the route for navigation', async () => {
  const { router, dispatch, calls, updates } = setup()
  router.attach()
  dispatch('mouseover', { target: linkEl('/about') })
  await settle()
  assert.equal(calls.length, 1, 'prefetch fetched /__data')
  await router.goto('/about')
  assert.equal(calls.length, 1, 'goto reused the prefetched data (no second fetch)')
  assert.deepEqual(updates.at(-1)?.data, { title: 'About' })
})

test('router: prefetch budget — max 5 in flight, dispose aborts them', async () => {
  const { router, dispatch, calls } = setup()
  router.attach()
  for (const h of ['/h1', '/h2', '/h3', '/h4', '/h5', '/h6', '/h7']) {
    dispatch('mouseover', { target: linkEl(h) })
  }
  assert.equal(calls.length, 5, 'only 5 prefetches started (budget)')
  router.dispose()
  assert.ok(calls.every((c) => c.signal?.aborted === true), 'all in-flight prefetches aborted')
})

test('router: hanging prefetch aborted on dispose', async () => {
  const { router, dispatch, calls } = setup()
  router.attach()
  dispatch('mouseover', { target: linkEl('/hang') })
  assert.equal(calls.length, 1)
  router.dispose()
  assert.equal(mustEl(calls, 0).signal?.aborted, true)
})

test('router: viewport prefetch via IntersectionObserver', async () => {
  const observed: unknown[] = []
  const cbs: Array<(entries: Array<{ target: unknown; isIntersecting: boolean }>) => void> = []
  class FakeIO {
    constructor(cb: (entries: Array<{ target: unknown; isIntersecting: boolean }>) => void) {
      cbs.push(cb)
    }
    observe(el: unknown) {
      observed.push(el)
    }
    disconnect() {}
  }
  const el = linkEl('/about')
  const { router, calls } = setup({ IntersectionObserver: FakeIO }, [el])
  router.attach()
  assert.equal(cbs.length, 1, 'IO created on attach')
  assert.deepEqual(observed, [el], 'internal link observed')
  mustCall(cbs, 0)([{ target: el, isIntersecting: true }])
  await settle()
  assert.equal(calls.length, 1, 'intersection triggered a prefetch')
  mustCall(cbs, 0)([{ target: el, isIntersecting: false }])
  assert.equal(calls.length, 1, 'non-intersecting entries ignored')
})

test('router: goto() routes the update through view transitions when available', async () => {
  const { router, vtCalls, updates } = setup()
  await router.goto('/about')
  assert.equal(vtCalls.length, 1, 'startViewTransition wrapped the update')
  assert.equal(updates.length, 1)
})

test('router: goto() rejects on non-2xx /__data responses', async () => {
  const { router } = setup({
    fetch: (() => Promise.resolve(new Response('nope', { status: 500 }))) as unknown as typeof fetch,
  })
  await assert.rejects(() => router.goto('/about'))
})

test('router: dataCache bounded at 64 entries, oldest evicted (BUG-15)', async () => {
  const { router, calls } = setup()
  router.attach()
  for (let i = 0; i < 70; i++) {
    await router.goto(`/p${i}`)
  }
  assert.equal(calls.length, 70, 'each fresh path fetched once')
  await router.goto('/p69')
  assert.equal(calls.length, 70, 'most-recent entry still cached (no refetch)')
  await router.goto('/p0')
  assert.equal(calls.length, 71, 'oldest entry evicted (refetched)')
})
