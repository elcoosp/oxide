// bootWithRouter tests (spec §8.4). This is the test the handoff asked for:
// a same-origin link click must (a) call `adapter.hydrate` on the SSR root
// BEFORE the router attaches, and (b) after that, dispatch `adapter.update`
// on navigation. The old failure mode was `createRouter().attach()` called
// with no preceding `adapter.hydrate`, which left the module-level `active`
// client handle undefined — URL changed but the DOM did not.
//
// The fake DOM is intentionally minimal; the "real DOM" check is exercised
// end-to-end by the smoke-app fixtures.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { bootWithRouter, type BootAdapter } from '../src/client/boot.ts'
import { componentFor, getRouteComponentSource, setRouteComponentSource } from '../src/client/route-manifest.ts'
import { encodeLite } from './helpers/odf-lite.ts'
import type { ClientInput, Disposer } from '../src/types.ts'

function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
}
function mustDef<T>(v: T | undefined, label: string): T {
  if (v === undefined) throw new Error(`mustDef: ${label} is undefined`)
  return v
}

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

function makeLink(href: string): any {
  const link: any = {
    addEventListener() {},
    removeEventListener() {},
    getAttribute(n: string) { return n === 'href' ? href : null },
    setAttribute() {},
    hasAttribute(n: string) { return n === 'href' },
    querySelector() { return null },
    querySelectorAll() { return [] },
  }
  link.closest = (sel: string) => (sel === 'a' ? link : null)
  return link
}

function makeDoc(opts: { root?: any; links?: any[] } = {}): any {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const links = opts.links ?? []
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
      if (sel.includes('a[href]')) return links
      return []
    },
    dispatchEvent() { return true },
    _clickCount: () => (listeners.get('click') ?? []).length,
    _listenerCount: (t: string) => (listeners.get(t) ?? []).length,
    _fireEvent(t: string, ev: unknown) {
      for (const f of listeners.get(t) ?? []) f(ev)
    },
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

function installFakes(opts: {
  root?: any
  links?: any[]
  oxide?: { route?: string; data?: string }
  oxideBoot?: { loadChunk: (c: string) => Promise<unknown> }
  fetchResult?: Uint8Array<ArrayBuffer>
}): { doc: any } {
  const doc = makeDoc({ root: opts.root, links: opts.links })
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

test('bootWithRouter: hydrates SSR root before attaching the router (§8.4)', () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const { doc } = installFakes({
      root,
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    let hydrateCalls = 0
    let hydrateInput: ClientInput | undefined
    const adapter: BootAdapter = {
      hydrate(_r: Element, i: ClientInput): Disposer { hydrateCalls++; hydrateInput = i; return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const clicksBefore = doc._clickCount()
    const dispose = bootWithRouter(adapter)
    assert.equal(hydrateCalls, 1, 'adapter.hydrate must be called exactly once')
    assert.ok(doc._clickCount() > clicksBefore, 'router click listener must be attached')
    assert.equal(mustDef(hydrateInput, 'hydrateInput').route, '/home')
    assert.deepEqual(mustDef(hydrateInput, 'hydrateInput').data, {})
    assert.equal(typeof dispose, 'function')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: does NOT attach the router when hydrate throws', () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const { doc } = installFakes({
      root,
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const adapter: BootAdapter = {
      hydrate() { throw new Error('hydrate boom') },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const clicksBefore = doc._clickCount()
    const dispose = bootWithRouter(adapter)
    assert.equal(doc._clickCount(), clicksBefore, 'no router click listener when hydrate throws')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: router-only attach when #root is missing but payload exists (§8.4 un-gating)', () => {
  const saved = saveGlobals()
  try {
    const { doc } = installFakes({
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const clicksBefore = doc._clickCount()
    const dispose = bootWithRouter(adapter)
    assert.ok(doc._clickCount() > clicksBefore, 'payload without #root attaches in router-only mode')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: does NOT attach the router when window.__oxide is missing', () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const { doc } = installFakes({
      root,
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const clicksBefore = doc._clickCount()
    const dispose = bootWithRouter(adapter)
    assert.equal(doc._clickCount(), clicksBefore, 'no router click listener without window.__oxide')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: router-only mode — attaches when payload exists without #root', () => {
  const saved = saveGlobals()
  try {
    const { doc } = installFakes({
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const clicksBefore = doc._clickCount()
    const dispose = bootWithRouter(adapter)
    assert.ok(doc._clickCount() > clicksBefore, 'router attaches without a root when payload exists')
    assert.ok(doc._listenerCount('ox:data') > 0, 'the ox:data HMR consumer is wired too')
    dispose()
    assert.equal(doc._listenerCount('ox:data'), 0, 'dispose removes the ox:data listener')
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: router-only mode navigates and dispatches adapter.update (§8.4)', async () => {
  const saved = saveGlobals()
  try {
    const link = makeLink('/about')
    const odf = encodeLite({ route: '/about', url: '/about', params: {}, data: { title: 'About' } })
    const { doc } = installFakes({
      links: [link],
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })
    const updates: any[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: unknown) { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter)
    doc._fireClick(link)
    await new Promise((r) => setTimeout(r, 30))
    assert.equal(updates.length, 1, 'router-only nav still reaches adapter.update')
    assert.equal(updates[0].route, '/about')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: ox:data (HMR route-data refresh) feeds adapter.update (§13.3)', () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const { doc } = installFakes({
      root,
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const updates: any[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: unknown) { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter)
    const input = { route: '/home', url: '/home', params: {}, data: { fresh: true } }
    doc._fireEvent('ox:data', { type: 'ox:data', detail: { route: '/home', input } })
    assert.equal(updates.length, 1, 'the HMR data event reaches the adapter')
    assert.deepEqual(updates[0], input)
    // Malformed detail is ignored.
    doc._fireEvent('ox:data', { type: 'ox:data', detail: {} })
    assert.equal(updates.length, 1)
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: boot-path defer glue wires the payload in router-only mode (§7.1/§9.2)', () => {
  const saved = saveGlobals()
  try {
    // The payload's deferred placeholder becomes a DeferredClient slot; the
    // injected `<script type="application/oxide-deferred">` resolves it.
    const { doc } = installFakes({
      oxide: { route: '/slow', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
    })
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    // No #root → router-only: the boot glue must attach and listen for swaps.
    const dispose = bootWithRouter(adapter)
    assert.ok(doc._listenerCount('ox:swap') > 0, 'boot glue listens for ox:swap without islands')
    dispose()
    assert.equal(doc._listenerCount('ox:swap'), 0, 'dispose tears the boot glue down')
  } finally {
    restoreGlobals(saved)
  }
})

test('bootWithRouter: registers the manifest as the adapters component source (§6.4)', async () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const { doc } = installFakes({
      root,
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async (c: string) => ({ default: () => c }) },
    })
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(): void {},
      hydrateIsland(): Disposer { return () => {} },
    }
    const manifest = { buildId: 'b1', routes: { 'routes/home/page': { entry_chunk: 'home.js' } } }
    const dispose = bootWithRouter(adapter, { manifest })
    const src = getRouteComponentSource()
    assert.ok(src, 'manifest registered')
    assert.equal(src!.manifest, manifest)
    const c = (await componentFor('/home')) as (s: string) => string
    assert.equal(c('/client/home.js'), '/client/home.js', 'componentFor resolves through the boot loader')
    dispose()
  } finally {
    restoreGlobals(saved)
    setRouteComponentSource(undefined)
  }
})

test('bootWithRouter: same-origin link click dispatches adapter.update (§8.4)', async () => {
  const saved = saveGlobals()
  try {
    const root = makeRoot()
    const link = makeLink('/about')
    const odf = encodeLite({ route: '/about', url: '/about', params: {}, data: { title: 'About' } })
    const { doc } = installFakes({
      root,
      links: [link],
      oxide: { route: '/home', data: '' },
      oxideBoot: { loadChunk: async () => ({}) },
      fetchResult: odf,
    })
    const updates: any[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: unknown) { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter)
    doc._fireClick(link)
    // goto() is async — let the microtask queue + fake fetch resolve.
    await new Promise((r) => setTimeout(r, 30))
    assert.equal(updates.length, 1, 'adapter.update must fire once after link click')
    assert.equal(updates[0].route, '/about')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})
