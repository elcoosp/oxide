// HMR client runtime (spec §13.3).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { applyHmrMessage, showOverlay, connectHmr } from '../src/client/hmr.ts'

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
