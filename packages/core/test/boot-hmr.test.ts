// §13.3: the inline dev HMR client dispatches `ox:data` with
// `detail: { route, odf }`. bootWithRouter must decode the base64 ODF and
// call `adapter.update` with a full ClientInput — otherwise a loader edit
// changes nothing on the page.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { bootWithRouter } from '../src/client/boot.ts'
import { encodeLite } from './helpers/odf-lite.ts'

function b64(bytes: Uint8Array): string {
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s)
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

function makeDoc(): any {
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
    getElementById(id: string) { return id === 'root' ? makeRoot() : null },
    querySelector() { return null },
    querySelectorAll() { return [] },
    dispatchEvent() { return true },
    createElement() { return { setAttribute() {}, remove() {}, appendChild() {} } },
    documentElement: { appendChild() {} },
    body: { appendChild() {} },
    _fire(t: string, e: unknown) {
      for (const f of listeners.get(t) ?? []) f(e)
    },
    _count(t: string) { return (listeners.get(t) ?? []).length },
  }
}

const saved: Record<string, unknown> = {}

function install(over: Record<string, unknown>): { doc: any } {
  const g = globalThis as Record<string, unknown>
  for (const k of ['window', 'document', 'location', 'history', '__oxide', '__oxideBoot']) {
    saved[k] = g[k]
  }
  const doc = makeDoc()
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const history: {
    state: unknown
    pushState(s: unknown): void
    replaceState(s: unknown): void
  } = {
    state: null as unknown,
    pushState(s) { this.state = s },
    replaceState(s) { this.state = s },
  }
  const w = {
    __oxide: { route: '/page', params: { slug: 'abc' }, data: '' },
    __oxideBoot: { loadChunk: async () => ({}) },
    location,
  }
  Object.assign(g, { window: w, document: doc, location, history, __oxide: w.__oxide, __oxideBoot: w.__oxideBoot }, over)
  return { doc }
}

function restore(): void {
  const g = globalThis as Record<string, unknown>
  for (const k of Object.keys(saved)) {
    const v = saved[k]
    if (v === undefined) delete g[k]
    else g[k] = v
  }
}

test('boot: ox:data with {route, odf} decodes and calls adapter.update', async () => {
  const { doc } = install({})
  try {
    const odfBytes = encodeLite({ route: '/page', url: '/page', params: {}, data: { title: 'Hot' } })
    const updates: Array<Record<string, unknown>> = []
    const adapter: any = {
      hydrate() {},
      update(i: any) { updates.push(i) },
      hydrateIsland() { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {} as never)

    doc._fire('ox:data', {
      type: 'ox:data',
      detail: { route: '/page', odf: b64(odfBytes) },
    })
    // onData is async — let it settle.
    await new Promise((r) => setTimeout(r, 10))

    assert.equal(updates.length, 1, 'adapter.update fired once')
    assert.equal(updates[0].route, '/page')
    assert.deepEqual(updates[0].data, { title: 'Hot' }, 'ODF decoded to the data payload')
    // window.__oxide.params threaded through.
    assert.deepEqual(updates[0].params, { slug: 'abc' })
    dispose()
  } finally {
    restore()
  }
})

test('boot: ox:data with a full {input} still works (unchanged shape)', async () => {
  const { doc } = install({})
  try {
    const updates: any[] = []
    const adapter: any = {
      hydrate() {}, update(i: any) { updates.push(i) }, hydrateIsland() { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {} as never)
    doc._fire('ox:data', {
      type: 'ox:data',
      detail: { input: { route: '/x', url: '/x', params: {}, data: { n: 1 } } },
    })
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(updates.length, 1)
    assert.equal(updates[0].route, '/x')
    dispose()
  } finally {
    restore()
  }
})

test('boot: ox:data with no route and no input is ignored', async () => {
  const { doc } = install({})
  try {
    const updates: any[] = []
    const adapter: any = {
      hydrate() {}, update(i: any) { updates.push(i) }, hydrateIsland() { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {} as never)
    doc._fire('ox:data', { type: 'ox:data', detail: {} })
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(updates.length, 0)
    dispose()
  } finally {
    restore()
  }
})
