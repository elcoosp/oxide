// §13.3: the inline dev HMR client dispatches `ox:data` with
// `detail: { route, odf }`. bootWithRouter must decode the base64 ODF and
// call `adapter.update` with a full ClientInput — otherwise a loader edit
// changes nothing on the page.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { bootWithRouter, type BootAdapter } from '../src/client/boot.ts'
import { encodeLite } from './helpers/odf-lite.ts'
import type { ClientInput, Disposer } from '../src/types.ts'

/** Typed indexed access with a runtime bounds check. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
}

function b64(bytes: Uint8Array): string {
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s)
}

interface FakeRoot {
  innerHTML: string
  addEventListener(type: string, fn: (e: unknown) => void): void
  removeEventListener(type: string, fn: (e: unknown) => void): void
  getAttribute(name: string): string | null
  setAttribute(name: string, value: string): void
  hasAttribute(name: string): boolean
  querySelector(sel: string): unknown
  querySelectorAll(sel: string): unknown[]
}

function makeRoot(): FakeRoot {
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

interface FakeDoc {
  addEventListener(type: string, fn: (e: unknown) => void): void
  removeEventListener(type: string, fn: (e: unknown) => void): void
  getElementById(id: string): FakeRoot | null
  querySelector(sel: string): unknown
  querySelectorAll(sel: string): unknown[]
  dispatchEvent(ev: unknown): boolean
  createElement(tag: string): { setAttribute(n: string, v: string): void; remove(): void; appendChild(c: unknown): void }
  documentElement: { appendChild(c: unknown): void }
  body: { appendChild(c: unknown): void }
  _fire(type: string, ev: unknown): void
  _count(type: string): number
}

function makeDoc(
  moduleScripts: string[] = [],
  fields: Array<Record<string, unknown>> = [],
): FakeDoc {
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
    querySelectorAll(sel: string) {
      // The selectors bootWithRouter consults: the module-script scan (the
      // ESM-dev probe, `script[type="module"]`) and the form-field scan (the
      // state restore). The loadScripts scan (`script[data-ox-script]`) must
      // NOT see the boot entry — it has no data-ox-script attribute.
      if (sel.includes('type="module"') || sel.includes("type='module'")) {
        return moduleScripts.map((src) => ({ getAttribute: (n: string) => (n === 'src' ? src : null) }))
      }
      if (sel.includes('input')) return fields
      return []
    },
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

function install(over: Record<string, unknown>): { doc: FakeDoc } {
  const g = globalThis as Record<string, unknown>
  for (const k of ['window', 'document', 'location', 'history', '__oxide', '__oxideBoot', 'sessionStorage', 'WebSocket']) {
    saved[k] = g[k]
  }
  const doc = makeDoc()
  const location = { href: 'http://x.test/', pathname: '/', search: '', protocol: 'http:', host: 'x.test' }
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
    // The §13.3 ESM-dev probe reads `window.document` (the boot ESM block
    // uses `maybeWindow()?.document`), so the fake window carries the doc.
    document: doc as unknown as Document,
  }
  Object.assign(g, { window: w, document: doc, location, history, __oxide: w.__oxide, __oxideBoot: w.__oxideBoot }, over)
  // Node 24 ships a real `WebSocket` client: without this stub the ESM-dev
  // boot path opens a genuine socket to ws://x.test (and its reconnect
  // timer keeps the event loop — and the test run — alive forever). Forced
  // (not `??`): the real global exists and must be shadowed.
  g.WebSocket = FakeSocket
  return { doc }
}

/** The hermetic socket double: records the URL, never fires events. */
class FakeSocket {
  static lastUrl: string | undefined
  constructor(url: string) {
    FakeSocket.lastUrl = url
  }
  close(): void {}
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
    const updates: ClientInput[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: ClientInput): void { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {})

    doc._fire('ox:data', {
      type: 'ox:data',
      detail: { route: '/page', odf: b64(odfBytes) },
    })
    // onData is async — let it settle.
    await new Promise((r) => setTimeout(r, 10))

    assert.equal(updates.length, 1, 'adapter.update fired once')
    const u0 = updates[0]
    if (u0 === undefined) throw new Error('test invariant: updates[0] should be defined')
    assert.equal(u0.route, '/page')
    assert.deepEqual(u0.data, { title: 'Hot' }, 'ODF decoded to the data payload')
    // window.__oxide.params threaded through.
    assert.deepEqual(u0.params, { slug: 'abc' })
    dispose()
  } finally {
    restore()
  }
})

test('boot: ox:data with a full {input} still works (unchanged shape)', async () => {
  const { doc } = install({})
  try {
    const updates: ClientInput[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: ClientInput): void { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {})
    doc._fire('ox:data', {
      type: 'ox:data',
      detail: { input: { route: '/x', url: '/x', params: {}, data: { n: 1 } } },
    })
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(updates.length, 1)
    assert.equal(mustEl(updates, 0).route, '/x')
    dispose()
  } finally {
    restore()
  }
})

test('boot: ox:data with no route and no input is ignored', async () => {
  const { doc } = install({})
  try {
    const updates: ClientInput[] = []
    const adapter: BootAdapter = {
      hydrate(): Disposer { return () => {} },
      update(input: ClientInput): void { updates.push(input) },
      hydrateIsland(): Disposer { return () => {} },
    }
    const dispose = bootWithRouter(adapter, {})
    doc._fire('ox:data', { type: 'ox:data', detail: {} })
    await new Promise((r) => setTimeout(r, 10))
    assert.equal(updates.length, 0)
    dispose()
  } finally {
    restore()
  }
})

// ---- V2-E01 phase 4: the fallback-reload stash restore on boot ----

interface FakeField {
  tagName: string
  attrs: Record<string, string>
  value: string
  getAttribute(k: string): string | null
  setAttribute(k: string, v: string): void
}

function fakeField(name: string, value = ''): FakeField {
  const attrs: Record<string, string> = { name }
  return {
    tagName: 'input',
    attrs,
    value,
    getAttribute(k: string) {
      return attrs[k] ?? null
    },
    setAttribute(k: string, v: string) {
      attrs[k] = v
    },
  }
}

function fakeStorage() {
  const map = new Map<string, string>()
  return {
    setItem: (k: string, v: string) => void map.set(k, String(v)),
    getItem: (k: string) => (map.has(k) ? (map.get(k) as string) : null),
    removeItem: (k: string) => void map.delete(k),
    has: (k: string) => map.has(k),
  }
}

function freshStash(value: string): string {
  return JSON.stringify({
    at: Date.now(),
    fields: [{ key: 'name:email', value, tag: 'input' }],
    focus: null,
  })
}

const BOOT_SCRIPT = '/@boot/entry.mjs'

function bootAdapter(): BootAdapter {
  return {
    hydrate(): Disposer {
      return () => {}
    },
    update(): void {},
    hydrateIsland(): Disposer {
      return () => {}
    },
  }
}

test('boot: the ESM-dev path restores the fallback-reload stash (V2-E01 phase 4)', async () => {
  const field = fakeField('email', '')
  const storage = fakeStorage()
  storage.setItem('ox-hmr-state', freshStash('typed-before-reload'))
  const { doc } = install({ sessionStorage: storage })
  // The ESM-dev probe: a module script served from /@boot/.
  const scripts = [BOOT_SCRIPT]
  ;(doc as unknown as { querySelectorAll(sel: string): unknown[] }).querySelectorAll = (sel: string) => {
    if (sel.includes('type="module"') || sel.includes("type='module'")) {
      return scripts.map((src) => ({ getAttribute: (n: string) => (n === 'src' ? src : null) }))
    }
    if (sel.includes('input')) return [field]
    return []
  }
  try {
    const dispose = bootWithRouter(bootAdapter(), {})
    // scheduleStashRestore: microtask pass + one 50 ms timer pass.
    await new Promise((r) => setTimeout(r, 60))
    assert.equal(field.value, 'typed-before-reload', 'the stashed input value came back')
    assert.equal(storage.has('ox-hmr-state'), false, 'the stash is cleared after restore')
    assert.equal(
      FakeSocket.lastUrl,
      'ws://x.test/_oxide/hmr',
      'the ESM-dev boot also wired the HMR socket',
    )
    dispose()
  } finally {
    restore()
  }
})

test('boot: a non-ESM boot leaves the fallback-reload stash alone', async () => {
  const field = fakeField('email', '')
  const storage = fakeStorage()
  storage.setItem('ox-hmr-state', freshStash('typed-before-reload'))
  const { doc } = install({ sessionStorage: storage })
  // Default fake doc: no module scripts → the production path.
  ;(doc as unknown as { querySelectorAll(sel: string): unknown[] }).querySelectorAll = (sel: string) => {
    if (sel.includes('input')) return [field]
    return []
  }
  try {
    const dispose = bootWithRouter(bootAdapter(), {})
    await new Promise((r) => setTimeout(r, 60))
    assert.equal(field.value, '', 'no restore outside the ESM dev path')
    assert.equal(storage.has('ox-hmr-state'), true, 'the stash is untouched')
    dispose()
  } finally {
    restore()
  }
})
