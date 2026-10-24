// §8.4 loading boundaries: `bootWithRouter` paints a route's loading
// component into an overlay on `onNavigateStart` and tears it down on
// `onNavigateEnd` (right before the tree swap). The framework adapter owns
// the actual render (React/Vue/Svelte/Solid); this test uses a stub
// `renderLoading` and asserts the router → boot plumbing: the right
// component is selected by URL prefix, the overlay div is appended, and the
// disposer is called.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { bootWithRouter, type BootAdapter } from '../src/client/boot.ts'
import { encodeLite } from './helpers/odf-lite.ts'
import type { ClientInput, Disposer } from '../src/types.ts'

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

    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
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
    })

    // Fire the click; goto() is async so we wait a microtask + the fetch.
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))

    assert.equal(renderLoadingCalls, 1, 'renderLoading fired once')
    assert.equal(loadingComponents[0], slowMarker, 'longest-prefix match wins (/slow/)')
    assert.equal(disposeCalls, 1, 'disposer called on nav end')

    // The overlay was created by createElement and removed by nav end.
    const overlays = doc._overlays
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
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} }, update(): void {}, hydrateIsland(): Disposer { return () => {} },
      renderLoading(): Disposer { renderLoadingCalls++; return () => {} },
    }

    const dispose = bootWithRouter(adapter, { loading: { '/slow/': {} } })
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
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} }, update(): void { updates++ }, hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter, { loading: { '/slow/': {} } })
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))
    assert.equal(updates, 1, 'navigation still completes')
    assert.equal((doc._overlays).length, 0, 'no overlay created without renderLoading')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

// ---------------------------------------------------------------------------
// Dynamic-segment keys (§8.4): the CLI emits `[name]` segment tokens for
// dynamic route dirs (`routes/blog/[slug]/loading` → `/blog/[slug]`). The
// matcher treats a `[name]` / `:name` / `_name_` key segment as "exactly one
// non-empty URL segment", keeps prefix semantics for literals, and picks the
// longest match (most segments; ties → more literal segments).
// ---------------------------------------------------------------------------

function makeLinkTo(href: string): any {
  const link: any = {
    addEventListener() {}, removeEventListener() {},
    getAttribute(n: string) { return n === 'href' ? href : null },
    setAttribute() {}, hasAttribute(n: string) { return n === 'href' },
    querySelector() { return null }, querySelectorAll() { return [] },
  }
  link.closest = (s: string) => (s === 'a' ? link : null)
  return link
}

/** Boot + click `path`; returns the loading components renderLoading saw. */
async function clickAndCollect(loading: Record<string, unknown>, path: string): Promise<unknown[]> {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const link = makeLinkTo(path)
    const odf = encodeLite({ route: path, url: path, params: {}, data: {} })
    const { doc } = install({
      root, link,
      oxide: { route: '/', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })
    const seen: unknown[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} }, update(): void {}, hydrateIsland(): Disposer { return () => {} },
      renderLoading(_o: unknown, _i: unknown, Component: unknown) {
        seen.push(Component)
        return () => {}
      },
    }
    const dispose = bootWithRouter(adapter, { loading })
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 20))
    dispose()
    return seen
  } finally {
    restoreGlobals(saved)
  }
}

const BLOG = { name: 'BlogLoading' }
const SLUG = { name: 'SlugLoading' }
const COMMENTS = { name: 'CommentsLoading' }
const ROOT = { name: 'RootLoading' }

test('loading boundary: [name] key matches exactly one URL segment', async () => {
  const seen = await clickAndCollect({ '/blog/[slug]': SLUG }, '/blog/hello')
  assert.equal(seen.length, 1, 'renderLoading fired')
  assert.equal(seen[0], SLUG, '/blog/[slug] matched /blog/hello')
})

test('loading boundary: [name] key does not match a URL missing that segment', async () => {
  const seen = await clickAndCollect({ '/blog/[slug]': SLUG }, '/blog')
  assert.equal(seen.length, 0, 'param segment needs its one URL segment')
})

test('loading boundary: :name and _name_ key spellings match like [name]', async () => {
  const colon = await clickAndCollect({ '/docs/:id': SLUG }, '/docs/42')
  assert.equal(colon.length, 1)
  assert.equal(colon[0], SLUG)
  const underscore = await clickAndCollect({ '/docs/_id_': SLUG }, '/docs/42')
  assert.equal(underscore.length, 1)
  assert.equal(underscore[0], SLUG)
})

test('loading boundary: longest key wins across a dynamic chain', async () => {
  const seen = await clickAndCollect(
    { '/blog': BLOG, '/blog/[slug]': SLUG, '/blog/[slug]/comments': COMMENTS },
    '/blog/hello/comments',
  )
  assert.equal(seen.length, 1)
  assert.equal(seen[0], COMMENTS, 'most segments wins')
})

test('loading boundary: shorter dynamic key wins over the static parent for its depth', async () => {
  const seen = await clickAndCollect({ '/blog': BLOG, '/blog/[slug]': SLUG }, '/blog/hello')
  assert.equal(seen.length, 1)
  assert.equal(seen[0], SLUG, '2-segment key beats the 1-segment prefix')
})

test('loading boundary: literal key prefix still covers deeper URLs', async () => {
  const seen = await clickAndCollect({ '/blog': BLOG }, '/blog/x/y/z')
  assert.equal(seen.length, 1)
  assert.equal(seen[0], BLOG, 'layout-level loading covers the routes below it')
})

test('loading boundary: trailing slash on a dynamic key is normalized', async () => {
  const seen = await clickAndCollect({ '/blog/[slug]/': SLUG }, '/blog/hello')
  assert.equal(seen.length, 1)
  assert.equal(seen[0], SLUG)
})

test('loading boundary: root key still matches everything, lowest priority', async () => {
  const everywhere = await clickAndCollect({ '/': ROOT }, '/anything/at/all')
  assert.equal(everywhere.length, 1)
  assert.equal(everywhere[0], ROOT)
  const fallback = await clickAndCollect({ '/': ROOT, '/blog/[slug]': SLUG }, '/blog/hello')
  assert.equal(fallback[0], SLUG, 'the deeper key beats the root key')
})

test('loading boundary: ties at the same depth resolve to more literal segments', async () => {
  // Both keys match /blog/hello at depth 2; the literal one is preferred.
  const seen = await clickAndCollect({ '/[x]/[y]': SLUG, '/blog/[slug]': BLOG }, '/blog/hello')
  assert.equal(seen.length, 1)
  assert.equal(seen[0], BLOG, 'more literal segments wins the tie')
})
