// §8.4 loading boundaries: `bootWithRouter` paints a route's loading
// component into an overlay on `onNavigateStart` and tears it down on
// `onNavigateEnd` (right before the tree swap). The framework adapter owns
// the actual render (React/Vue/Svelte/Solid); this test uses a stub
// `renderLoading` and asserts the router → boot plumbing: the right
// component is selected by URL prefix, the overlay div is appended, and the
// disposer is called.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { bootWithRouter } from '../src/client/boot.ts'
import { encodeLite } from './helpers/odf-lite.ts'

interface Saved {
  window?: unknown
  document?: unknown
  location?: unknown
  history?: unknown
  fetch?: unknown
  __oxide?: unknown
  __oxideBoot?: unknown
}

function saveGlobals(): Saved {
  const g = globalThis as Record<string, unknown>
  return {
    window: g.window,
    document: g.document,
    location: g.location,
    history: g.history,
    fetch: g.fetch,
    __oxide: g.__oxide,
    __oxideBoot: g.__oxideBoot,
  }
}

function restoreGlobals(s: Saved): void {
  const g = globalThis as Record<string, unknown>
  for (const k of Object.keys(s)) {
    const v = (s as Record<string, unknown>)[k]
    if (v === undefined) delete g[k]
    else g[k] = v
  }
}

function makeRoot(): any {
  return {
    innerHTML: '<p>ssr</p>',
    addEventListener() {},
    removeEventListener() {},
    getAttribute() { return null },
    setAttribute() {},
    hasAttribute() { return false },
    querySelector() { return null },
    querySelectorAll() { return [] },
  }
}

function makeOverlay(): any {
  return {
    tagName: 'DIV',
    attrs: {} as Record<string, string>,
    removed: false,
    setAttribute(k: string, v: string) { this.attrs[k] = v },
    getAttribute(k: string) { return this.attrs[k] ?? null },
    remove() { this.removed = true },
    appendChild(_: unknown) {},
  }
}

function makeDoc(opts: { root?: any; link?: any } = {}): any {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const overlays: any[] = []
  const doc: any = {
    addEventListener(t: string, f: (e: unknown) => void) {
      const l = listeners.get(t) ?? []
      l.push(f)
      listeners.set(t, l)
    },
    removeEventListener(t: string, f: (e: unknown) => void) {
      listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f))
    },
    getElementById(id: string) {
      return id === 'root' ? opts.root ?? null : null
    },
    querySelector() { return null },
    querySelectorAll(sel: string) {
      return sel.includes('a[href]') && opts.link ? [opts.link] : []
    },
    dispatchEvent() { return true },
    createElement(tag: string) {
      const el = makeOverlay()
      el.tagName = tag.toUpperCase()
      overlays.push(el)
      return el
    },
    documentElement: {
      appendChild(el: any) { void el },
    },
    body: {
      appendChild(el: any) { void el },
    },
    _overlays: overlays,
    _fireClick(link: any) {
      const ev = {
        target: { closest: (s: string) => (s === 'a' ? link : null) },
        button: 0, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false,
        preventDefault() {},
      }
      for (const f of listeners.get('click') ?? []) f(ev)
    },
  }
  return doc
}

function install(opts: {
  root?: any
  link?: any
  oxide?: { route?: string; data?: string }
  oxideBoot?: { loadChunk: (c: string) => Promise<unknown> }
  fetchResult?: Uint8Array
}): { doc: any } {
  const doc = makeDoc({ root: opts.root, link: opts.link })
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const history = {
    state: null as unknown,
    pushState(s: unknown) { this.state = s },
    replaceState(s: unknown) { this.state = s },
  }
  const g = globalThis as Record<string, unknown>
  g.window = { __oxide: opts.oxide, __oxideBoot: opts.oxideBoot, location }
  g.document = doc
  g.location = location
  g.history = history
  g.__oxide = opts.oxide
  g.__oxideBoot = opts.oxideBoot
  if (opts.fetchResult) {
    g.fetch = async () =>
      new Response(opts.fetchResult as unknown as BodyInit, { status: 200 })
  }
  return { doc }
}

test('loading boundary: renderLoading paints an overlay on nav start', async () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const link: any = {
      addEventListener() {}, removeEventListener() {},
      getAttribute(n: string) { return n === 'href' ? '/slow' : null },
      setAttribute() {}, hasAttribute(n: string) { return n === 'href' },
      querySelector() { return null }, querySelectorAll() { return [] },
    }
    link.closest = (s: string) => (s === 'a' ? link : null)

    const odf = encodeLite({ route: '/slow', url: '/slow', params: {}, data: {} })
    const { doc } = install({
      root, link,
      oxide: { route: '/', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })

    // Adapter stub: hydrate no-op; renderLoading paints an overlay that
    // we can observe, and records that its disposer is called.
    let renderLoadingCalls = 0
    let disposeCalls = 0
    const loadingComponents: unknown[] = []
    const slowMarker = { name: 'SlowLoading' }
    const aboutMarker = { name: 'AboutLoading' }

    const adapter: any = {
      hydrate() {},
      update() {},
      hydrateIsland() { return () => {} },
      renderLoading(overlay: any, input: any, Component: any) {
        renderLoadingCalls++
        loadingComponents.push(Component)
        overlay.setAttribute('data-rendered', '1')
        return () => { disposeCalls++ }
      },
    }

    const dispose = bootWithRouter(adapter, {
      loading: {
        '/': aboutMarker,
        '/slow/': slowMarker,
      },
    } as never)

    // Fire the click; goto() is async so we wait a microtask + the fetch.
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))

    assert.equal(renderLoadingCalls, 1, 'renderLoading fired once')
    assert.equal(loadingComponents[0], slowMarker, 'longest-prefix match wins (/slow/)')
    assert.equal(disposeCalls, 1, 'disposer called on nav end')

    // The overlay was created by createElement and removed by nav end.
    const overlays = doc._overlays as any[]
    assert.equal(overlays.length, 1, 'one overlay created')
    assert.equal(overlays[0].removed, true, 'overlay removed on nav end')

    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('loading boundary: no loading component → renderLoading not called', async () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const link: any = {
      addEventListener() {}, removeEventListener() {},
      getAttribute(n: string) { return n === 'href' ? '/other' : null },
      setAttribute() {}, hasAttribute(n: string) { return n === 'href' },
      querySelector() { return null }, querySelectorAll() { return [] },
    }
    link.closest = (s: string) => (s === 'a' ? link : null)
    const odf = encodeLite({ route: '/other', url: '/other', params: {}, data: {} })
    const { doc } = install({
      root, link,
      oxide: { route: '/', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })

    let renderLoadingCalls = 0
    const adapter: any = {
      hydrate() {}, update() {}, hydrateIsland() { return () => {} },
      renderLoading() { renderLoadingCalls++; return () => {} },
    }

    const dispose = bootWithRouter(adapter, { loading: { '/slow/': {} } } as never)
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))
    assert.equal(renderLoadingCalls, 0, 'no matching prefix → no overlay')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('loading boundary: no adapter.renderLoading → hooks are no-ops', async () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const link: any = {
      addEventListener() {}, removeEventListener() {},
      getAttribute(n: string) { return n === 'href' ? '/slow' : null },
      setAttribute() {}, hasAttribute(n: string) { return n === 'href' },
      querySelector() { return null }, querySelectorAll() { return [] },
    }
    link.closest = (s: string) => (s === 'a' ? link : null)
    const odf = encodeLite({ route: '/slow', url: '/slow', params: {}, data: {} })
    const { doc } = install({
      root, link,
      oxide: { route: '/', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })

    // No renderLoading on the adapter — the router still navigates, the
    // overlay is just not rendered.
    let updates = 0
    const adapter: any = {
      hydrate() {}, update() { updates++ }, hydrateIsland() { return () => {} },
    }
    const dispose = bootWithRouter(adapter, { loading: { '/slow/': {} } } as never)
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))
    assert.equal(updates, 1, 'navigation still completes')
    assert.equal((doc._overlays as any[]).length, 0, 'no overlay created without renderLoading')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})
