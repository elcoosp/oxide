// HMR client runtime (spec §13.3).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { applyHmrMessage, showOverlay, connectHmr } from '../src/client/hmr.ts'
import { encodeLite } from './helpers/odf-lite.ts'

function fakeDoc() {
  const byId: Record<string, FakeEl> = {}
  const links: FakeEl[] = []
  const make = (tag: string): FakeEl => ({
    tagName: tag,
    attrs: {},
    style: { cssText: '' },
    children: [] as FakeEl[],
    innerHTML: '',
    textContent: '',
    id: '',
    setAttribute(k: string, v: string) { this.attrs[k] = v },
    getAttribute(k: string) { return this.attrs[k] ?? null },
    appendChild(c: FakeEl) { this.children.push(c); if (c.id) byId[c.id] = c; return c },
    addEventListener() {},
    remove() {},
  })
  const doc = {
    body: make('body'),
    createElement: (t: string) => make(t),
    getElementById: (id: string) => byId[id] ?? null,
    querySelectorAll: (_s: string) => links,
  }
  return { doc, links, make }
}

interface FakeEl {
  tagName: string
  attrs: Record<string, string>
  style: { cssText: string }
  children: FakeEl[]
  innerHTML: string
  textContent: string
  id: string
  setAttribute(k: string, v: string): void
  getAttribute(k: string): string | null
  appendChild(c: FakeEl): FakeEl
  addEventListener(...a: unknown[]): void
  remove(): void
}

test('css-update swaps the matching stylesheet href', () => {
  const saved = (globalThis as Record<string, unknown>).document
  const { doc, links, make } = fakeDoc()
  const link = make('link')
  link.setAttribute('rel', 'stylesheet')
  link.setAttribute('href', '/styles/app.css')
  links.push(link)
  ;(globalThis as Record<string, unknown>).document = doc
  try {
    let seen = ''
    const handled = applyHmrMessage({ type: 'css-update', path: '/styles/app.css' }, { onCssUpdate: (p) => { seen = p } })
    assert.equal(handled, true)
    assert.ok((link.getAttribute('href') ?? '').startsWith('/styles/app.css?t='), 'href cache-busted')
    assert.equal(seen, '/styles/app.css')
  } finally {
    if (saved === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = saved
  }
})

test('route-data-update calls onRouteData with the ODF payload', () => {
  const saved = (globalThis as Record<string, unknown>).document
  const { doc } = fakeDoc()
  ;(globalThis as Record<string, unknown>).document = doc
  try {
    let got: [string, string] = ['', '']
    applyHmrMessage({ type: 'route-data-update', route: '/x', odf_b64: 'T0RGMQ==' }, { onRouteData: (r, o) => { got = [r, o] } })
    assert.deepEqual(got, ['/x', 'T0RGMQ=='])
  } finally {
    if (saved === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = saved
  }
})

test('error message renders the overlay with code + fix', () => {
  const saved = (globalThis as Record<string, unknown>).document
  const { doc } = fakeDoc()
  ;(globalThis as Record<string, unknown>).document = doc
  try {
    const el = showOverlay({ code: 'CFG-BAD', message: 'boom', fix: 'rename it' })
    assert.ok(el, 'overlay element returned')
    assert.ok(el!.innerHTML !== undefined)
    assert.ok(el!.children.length >= 2, 'header + body')
  } finally {
    if (saved === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = saved
  }
})

test('full-reload calls the overridden reload', () => {
  const saved = (globalThis as Record<string, unknown>).document
  const { doc } = fakeDoc()
  ;(globalThis as Record<string, unknown>).document = doc
  try {
    let reloaded = false
    applyHmrMessage({ type: 'full-reload', reason: 'config' }, { reload: () => { reloaded = true } })
    assert.equal(reloaded, true)
  } finally {
    if (saved === undefined) delete (globalThis as Record<string, unknown>).document
    else (globalThis as Record<string, unknown>).document = saved
  }
})

test('connectHmr is a no-op outside the browser', () => {
  const dispose = connectHmr()
  assert.equal(typeof dispose, 'function')
  dispose()
})

// ---------------------------------------------------------------------------
// §13.3 route-data-update → ox:data: a loader edit re-fetches /__data (or
// decodes the pushed odf_b64) and dispatches the same event bootWithRouter
// consumes to call adapter.update. The fake doc records dispatched events.
// ---------------------------------------------------------------------------

interface DispatchedEvent {
  type: string
  detail?: {
    route?: string
    input?: { route: string; url: string; params: Record<string, string>; data: unknown }
  }
}

function fakeDocWithDispatch() {
  const dispatched: DispatchedEvent[] = []
  const base = fakeDoc()
  const doc = Object.assign(base.doc, {
    dispatchEvent(ev: DispatchedEvent) {
      dispatched.push(ev)
      return true
    },
  })
  return { doc, dispatched }
}

function installHmrGlobals(opts: { doc: unknown; location?: unknown; fetch?: unknown } = {}): () => void {
  const g = globalThis as Record<string, unknown>
  const saved = { document: g.document, location: g.location, fetch: g.fetch }
  g.document = opts.doc
  if (opts.location !== undefined) g.location = opts.location
  if (opts.fetch !== undefined) g.fetch = opts.fetch
  return () => {
    if (saved.document === undefined) delete g.document
    else g.document = saved.document
    if (saved.location === undefined) delete g.location
    else g.location = saved.location
    if (saved.fetch === undefined) delete g.fetch
    else g.fetch = saved.fetch
  }
}

test('route-data-update with a pushed payload decodes it and dispatches ox:data', async () => {
  const { doc, dispatched } = fakeDocWithDispatch()
  const restore = installHmrGlobals({
    doc,
    location: { pathname: '/slow', search: '' },
  })
  try {
    const odf = encodeLite({ route: '/slow', url: '/slow', params: { slug: 'x' }, data: { items: [1, 2] } })
    const b64 = Buffer.from(odf).toString('base64')
    const handled = applyHmrMessage({ type: 'route-data-update', route: 'src/routes/slow/page.server.ts', odf_b64: b64 })
    assert.equal(handled, true)
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(dispatched.length, 1, 'one ox:data dispatched')
    assert.equal(dispatched[0].type, 'ox:data')
    assert.equal(dispatched[0].detail?.route, '/slow')
    assert.deepEqual(dispatched[0].detail?.input?.data, { items: [1, 2] })
    assert.deepEqual(dispatched[0].detail?.input?.params, { slug: 'x' })
  } finally {
    restore()
  }
})

test('route-data-update without a payload re-fetches /__data for the current route', async () => {
  const { doc, dispatched } = fakeDocWithDispatch()
  const fetched: string[] = []
  const odf = encodeLite({ route: '/slow', url: '/slow', params: {}, data: { n: 7 } })
  const restore = installHmrGlobals({
    doc,
    location: { pathname: '/slow', search: '?x=1' },
    fetch: (async (url: string) => {
      fetched.push(url)
      return new Response(odf as unknown as BodyInit, { status: 200 })
    }) as typeof fetch,
  })
  try {
    applyHmrMessage({ type: 'route-data-update', route: 'src/routes/slow/page.server.ts', odf_b64: '' })
    await new Promise((r) => setTimeout(r, 10))
    assert.deepEqual(fetched, ['/__data?route=%2Fslow%3Fx%3D1'], 'current path (+search) fetched')
    assert.equal(dispatched.length, 1)
    assert.equal(dispatched[0].detail?.input?.data && (dispatched[0].detail!.input!.data as { n?: number }).n, 7)
  } finally {
    restore()
  }
})

test('route-data-update is silent on a failed /__data fetch', async () => {
  const { doc, dispatched } = fakeDocWithDispatch()
  const restore = installHmrGlobals({
    doc,
    location: { pathname: '/x', search: '' },
    fetch: (async () => new Response('nope', { status: 500 })) as typeof fetch,
  })
  try {
    const handled = applyHmrMessage({ type: 'route-data-update', route: 'p', odf_b64: '' })
    assert.equal(handled, true, 'the message itself is handled synchronously')
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(dispatched.length, 0, 'no event on a failed refresh')
  } finally {
    restore()
  }
})

test('route-data-update: malformed pushed odf_b64 is ignored', async () => {
  const { doc, dispatched } = fakeDocWithDispatch()
  const restore = installHmrGlobals({ doc, location: { pathname: '/x', search: '' } })
  try {
    applyHmrMessage({ type: 'route-data-update', route: 'p', odf_b64: '!!!not-base64-odf!!!' })
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(dispatched.length, 0)
  } finally {
    restore()
  }
})
