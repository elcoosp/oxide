// `<Script>` loading strategies (spec §16.6).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { scheduleScript, loadScripts, type ScriptOptions } from '../src/client/scripts.ts'

interface FakeEl {
  tagName: string
  type?: string
  src?: string
  textContent?: string
  attrs: Record<string, string>
  listeners: Record<string, Array<(e?: unknown) => void>>
  setAttribute(k: string, v: string): void
  getAttribute(k: string): string | null
  addEventListener(t: string, fn: (e?: unknown) => void, o?: unknown): void
}

function fakeDoc() {
  const head: FakeEl[] = []
  const make = (tag: string): FakeEl => ({
    tagName: tag,
    attrs: {},
    listeners: {},
    setAttribute(k, v) { this.attrs[k] = v },
    getAttribute(k) { return this.attrs[k] ?? null },
    addEventListener(t, fn) { (this.listeners[t] ??= []).push(fn) },
  })
  const doc = {
    readyState: 'complete' as string,
    head: { appendChild: (el: FakeEl) => { head.push(el) } },
    currentScript: null as FakeEl | null,
    listeners: {} as Record<string, Array<() => void>>,
    addEventListener(t: string, fn: () => void) { (this.listeners[t] ??= []).push(fn) },
    querySelector: (_s: string) => null as FakeEl | null,
    querySelectorAll: (_s: string) => [] as FakeEl[],
    createElement: (t: string) => make(t),
  }
  return { doc, head }
}

/** Typed indexed access with a runtime bounds check. Throws a clear error
 * (failing the test) instead of returning `T | undefined` — the honest way
 * to satisfy `noUncheckedIndexedAccess` in tests whose fixtures are known
 * non-empty. No casts, no `any`, no `!`, no `?? default`. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
}

test('scheduleScript("defer") injects a src script into head', () => {
  const { doc, head } = fakeDoc()
  let loaded = false
  scheduleScript('defer', { doc: doc as never, src: '/a.js', onLoad: () => { loaded = true } })
  assert.equal(head.length, 1)
  assert.equal(mustEl(head, 0).src, '/a.js')
  // `onLoad` is bound to the element's `load` event for a src script.
  for (const fn of mustEl(head, 0).listeners['load'] ?? []) fn()
  assert.equal(loaded, true)
})

test('a module script carries type="module"', () => {
  const { doc, head } = fakeDoc()
  scheduleScript('defer', { doc: doc as never, src: '/m.js', module: true })
  assert.equal(mustEl(head, 0).type, 'module')
})

test('inline content becomes textContent, not a src', () => {
  const { doc, head } = fakeDoc()
  scheduleScript('defer', { doc: doc as never, content: 'console.log(1)' })
  assert.equal(mustEl(head, 0).textContent, 'console.log(1)')
  assert.equal(mustEl(head, 0).src, undefined)
})

test('attrs are copied onto the injected element', () => {
  const { doc, head } = fakeDoc()
  scheduleScript('defer', { doc: doc as never, src: '/a.js', attrs: { 'data-x': 'y', crossorigin: 'anonymous' } })
  assert.equal(mustEl(head, 0).attrs['data-x'], 'y')
  assert.equal(mustEl(head, 0).attrs['crossorigin'], 'anonymous')
})

test('scheduleScript is a no-op with no document (SSR)', () => {
  const saved = (globalThis as Record<string, unknown>).document
  delete (globalThis as Record<string, unknown>).document
  try {
    const dispose = scheduleScript('defer', { src: '/a.js' })
    assert.equal(typeof dispose, 'function')
    dispose() // no throw
  } finally {
    if (saved !== undefined) (globalThis as Record<string, unknown>).document = saved
  }
})

test('scheduleScript("worker") runs the body in a Worker, not the main thread', async () => {
  const { doc, head } = fakeDoc()
  let workerBody = ''
  class FakeWorker {
    constructor(url: string) { void url }
    postMessage() {}
    terminate() {}
  }
  const savedBlob = globalThis.Blob
  const savedURL = globalThis.URL
  const savedWorker = (globalThis as Record<string, unknown>).Worker
  // Capture the Blob body.
  Reflect.set(globalThis, 'Blob', class {
    body: string
    constructor(parts: string[]) { this.body = parts.join('') }
  })
  Reflect.set(globalThis, 'URL', {
    createObjectURL(b: { body: string }) { workerBody = b.body; return 'blob:x' },
    revokeObjectURL() {},
  })
  Reflect.set(globalThis, 'Worker', FakeWorker)
  try {
    scheduleScript('worker', { doc: doc as never, content: 'postMessage(1)' })
    // `idle` falls back to setTimeout(1); wait for it, THEN assert — the
    // globals must stay installed until the deferred callback runs.
    await new Promise<void>((resolve) => setTimeout(resolve, 15))
    assert.equal(head.length, 0, 'a worker script must not touch the DOM')
    assert.equal(workerBody, 'postMessage(1)')
  } finally {
    Reflect.set(globalThis, 'Blob', savedBlob)
    Reflect.set(globalThis, 'URL', savedURL)
    if (savedWorker === undefined) delete (globalThis as Record<string, unknown>).Worker
    else Reflect.set(globalThis, 'Worker', savedWorker)
  }
})

test('scheduleScript("lazy") waits for intersection', () => {
  const { doc, head } = fakeDoc()
  const placeholder = doc.createElement('div')
  doc.currentScript = placeholder
  let cb: ((entries: Array<{ isIntersecting: boolean }>) => void) | undefined
  class FakeIO {
    constructor(fn: typeof cb) { cb = fn }
    observe() {}
    disconnect() {}
  }
  const dispose = scheduleScript('lazy', {
    doc: doc as never,
    src: '/lazy.js',
    IntersectionObserver: FakeIO as never,
  })
  assert.equal(head.length, 0, 'not injected before intersection')
  cb?.([{ isIntersecting: true }])
  assert.equal(head.length, 1)
  dispose()
})

test('loadScripts scans data-ox-script placeholders and schedules them', () => {
  const { doc, head } = fakeDoc()
  const tag = doc.createElement('script')
  tag.attrs['data-ox-script'] = ''
  tag.attrs['data-ox-strategy'] = 'defer'
  tag.attrs['data-ox-src'] = '/from-tag.js'
  ;(doc as unknown as { querySelectorAll: () => FakeEl[] }).querySelectorAll = () => [tag]
  loadScripts({ doc: doc as never })
  assert.equal(head.length, 1)
  assert.equal(mustEl(head, 0).src, '/from-tag.js')
})

test('loadScripts decodes an inline base64 body', () => {
  const { doc, head } = fakeDoc()
  const code = 'window.__x=1'
  const b64 = Buffer.from(code, 'utf8').toString('base64')
  const tag = doc.createElement('script')
  tag.attrs['data-ox-script'] = ''
  tag.attrs['data-ox-inline'] = b64
  ;(doc as unknown as { querySelectorAll: () => FakeEl[] }).querySelectorAll = () => [tag]
  loadScripts({ doc: doc as never })
  assert.equal(mustEl(head, 0).textContent, code)
})
