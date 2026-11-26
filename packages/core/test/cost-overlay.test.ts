// §V2-F17 cost overlay pins (the recorded remaining half of the F17 row:
// "the overlay UI remains"). The server wire (`GET /__oxide/cost?src=<path>`,
// dev-token guarded) and the compiler's `data-ox-src="path:line"` stamps are
// landed; these pins drive the client overlay that consumes both, before the
// module exists.
//
// The spec's acceptance, pinned here V8-free (the smoke-app hover is the
// Playwright job's):
// - activation is dev-only: `?ox-cost` in the URL or Alt+Shift+O;
// - the overlay highlights the hovered element's component and lists the
//   three heaviest;
// - Svelte/Solid show bytes only "and the overlay says so" — the
//   "render time unavailable" note renders off a null `renderMsP50`;
// - the fetch speaks the endpoint contract (`src=`, the `token=` the
//   dev-token guard parses) and degrades LOUD on 403/404/network — never a
//   fabricated record.
//
// The fake DOM is intentionally minimal (the boot.test.ts convention): the
// "real DOM" check is exercised end-to-end by the smoke-app fixtures.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  costOverlayActivatable,
  costPanelPlan,
  formatBytes,
  heaviest,
  mountCostOverlay,
  parseOxSrc,
  type ComponentCostRecord,
} from '../src/client/cost-overlay.ts'

// ------------------------------------------------------------- parseOxSrc

test('parseOxSrc splits the stamp into path and line', () => {
  assert.deepEqual(parseOxSrc('src/routes/blog/[slug]/page.tsx:1'), {
    path: 'src/routes/blog/[slug]/page.tsx',
    line: 1,
  })
  assert.deepEqual(parseOxSrc('src/routes/index.tsx:12'), {
    path: 'src/routes/index.tsx',
    line: 12,
  })
})

test('parseOxSrc refuses stamps it cannot attribute', () => {
  // The stamp is `path:line` — no colon, a non-numeric line, or an empty
  // path is a stamp the overlay cannot attribute; it must say so (undefined)
  // rather than guess a path.
  assert.equal(parseOxSrc(undefined), undefined)
  assert.equal(parseOxSrc(null), undefined)
  assert.equal(parseOxSrc(''), undefined)
  assert.equal(parseOxSrc('src/routes/index.tsx'), undefined)
  assert.equal(parseOxSrc('src/routes/index.tsx:not-a-line'), undefined)
  assert.equal(parseOxSrc(':1'), undefined)
})

// --------------------------------------------------- costOverlayActivatable

test('the overlay activates on the ?ox-cost query spelling', () => {
  assert.equal(costOverlayActivatable('?ox-cost'), true)
  assert.equal(costOverlayActivatable('?ox-cost=1'), true)
  // A value the boot passes beside it must not break the activation.
  assert.equal(costOverlayActivatable('?route=/blog&ox-cost'), true)
})

test('the overlay stays off without the query', () => {
  assert.equal(costOverlayActivatable(''), false)
  assert.equal(costOverlayActivatable('?route=/blog'), false)
  // Prefix collisions are not the activation (`?ox-costly` is not `?ox-cost`).
  assert.equal(costOverlayActivatable('?ox-costly=1'), false)
})

// ----------------------------------------------------------------- heaviest

const rec = (bytes: number, over: Partial<ComponentCostRecord> = {}): ComponentCostRecord => ({
  bytes,
  gzip: Math.round(bytes / 3),
  chunk: 'boot-abc.js',
  renderMsP50: null,
  cache: null,
  ...over,
})

test('heaviest lists the n largest by bytes', () => {
  const rows = [rec(100, { chunk: 'a' }), rec(900, { chunk: 'b' }), rec(300, { chunk: 'c' }), rec(500, { chunk: 'd' })]
  const top = heaviest(rows, 3)
  assert.deepEqual(
    top.map((r) => r.chunk),
    ['b', 'd', 'c'],
  )
})

test('heaviest keeps the first-seen order on equal bytes and tolerates n past the length', () => {
  const rows = [rec(10, { chunk: 'first' }), rec(10, { chunk: 'second' }), rec(10, { chunk: 'third' })]
  assert.deepEqual(
    heaviest(rows, 3).map((r) => r.chunk),
    ['first', 'second', 'third'],
  )
  // n beyond the length is all of them (the list is honest, never padded).
  assert.deepEqual(
    heaviest(rows, 5).map((r) => r.chunk),
    ['first', 'second', 'third'],
  )
  assert.deepEqual(heaviest([], 3), [])
})

// -------------------------------------------------------------- formatBytes

test('formatBytes speaks the panel units', () => {
  assert.equal(formatBytes(970), '970 B')
  assert.equal(formatBytes(1234), '1.2 KB')
  assert.equal(formatBytes(12 * 1024 * 1024), '12.0 MB')
})

// ------------------------------------------------------------ costPanelPlan

test('the panel plan renders the record and the bytes-only note', () => {
  const withRender = costPanelPlan(
    rec(2048, { gzip: 683, chunk: 'boot-abc.js', renderMsP50: 12.34, cache: 'L0' }),
  )
  assert.ok(withRender.some((l) => l.includes('2.0 KB')), 'bytes line: {withRender}')
  assert.ok(withRender.some((l) => l.includes('683 B')), 'gzip line: {withRender}')
  assert.ok(withRender.some((l) => l.includes('boot-abc.js')), 'chunk line: {withRender}')
  assert.ok(withRender.some((l) => l.includes('12.3 ms')), 'p50 line: {withRender}')
  assert.ok(withRender.some((l) => l.includes('L0')), 'cache line: {withRender}')
  // The spec's note: "Svelte and Solid show bytes only, and the overlay
  // says so" — a null renderMsP50 renders the note, not a fabricated 0.
  const bytesOnly = costPanelPlan(rec(512, { renderMsP50: null }))
  assert.ok(
    bytesOnly.some((l) => l.includes('render time unavailable')),
    'the bytes-only note: {bytesOnly}',
  )
  assert.equal(bytesOnly.some((l) => l.includes('ms')), false)
})

// --------------------------------------------------------- mountCostOverlay

interface Saved {
  window?: unknown
  document?: unknown
  fetch?: unknown
}
function saveGlobals(): Saved {
  const g = globalThis as Record<string, unknown>
  return { window: g.window, document: g.document, fetch: g.fetch }
}
function restoreGlobals(s: Saved): void {
  const g = globalThis as Record<string, unknown>
  for (const k of Object.keys(s)) {
    if (s[k] === undefined) delete g[k]
    else g[k] = s[k]
  }
}

// A minimal fake element: attrs + children + the two DOM moves the overlay
// uses (`closest` walks `parent`, `remove` unlinks).
interface FakeEl {
  tag: string
  attrs: Map<string, string>
  children: FakeEl[]
  parent?: FakeEl
  removed: boolean
  textContent: string
  listeners: Map<string, Array<(ev?: unknown) => void>>
}
function el(tag: string, attrs: Record<string, string> = {}, parent?: FakeEl): FakeEl {
  const e: FakeEl = {
    tag,
    attrs: new Map(Object.entries(attrs)),
    children: [],
    parent,
    removed: false,
    textContent: '',
    listeners: new Map(),
  }
  // The DOM moves the overlay uses, spoken over the attrs map.
  const anyE = e as unknown as Record<string, unknown>
  anyE.getAttribute = (name: string): string | null => e.attrs.get(name) ?? null
  anyE.setAttribute = (name: string, value: string): void => {
    e.attrs.set(name, value)
  }
  anyE.removeAttribute = (name: string): void => {
    e.attrs.delete(name)
  }
  anyE.appendChild = (child: FakeEl): FakeEl => {
    child.parent = e
    e.children.push(child)
    return child
  }
  anyE.remove = (): void => {
    e.removed = true
  }
  if (parent) parent.children.push(e)
  return e
}

function fakeDoc() {
  const listeners = new Map<string, Array<(ev?: unknown) => void>>()
  const created: FakeEl[] = []
  const head = el('head')
  const body = el('body')
  const documentElement = el('html')
  const doc = {
    documentElement,
    head,
    body,
    readyState: 'complete',
    createElement(tag: string): FakeEl {
      const e = el(tag)
      created.push(e)
      return e
    },
    querySelectorAll(sel: string): FakeEl[] {
      // The overlay's only selector form is `[attr]` — match the bare attr name.
      const name = sel.startsWith('[') && sel.endsWith(']') ? sel.slice(1, -1) : sel
      const found: FakeEl[] = []
      const walk = (node: FakeEl): void => {
        if (node.attrs.has(name)) found.push(node)
        for (const child of node.children) walk(child)
      }
      walk(documentElement)
      walk(head)
      walk(body)
      return found
    },
    addEventListener(kind: string, cb: (ev?: unknown) => void): void {
      const list = listeners.get(kind) ?? []
      list.push(cb)
      listeners.set(kind, list)
    },
    removeEventListener(kind: string, cb: (ev?: unknown) => void): void {
      listeners.set(
        kind,
        (listeners.get(kind) ?? []).filter((f) => f !== cb),
      )
    },
  }
  const dispatch = (kind: string, ev?: unknown): void => {
    for (const cb of listeners.get(kind) ?? []) cb(ev)
  }
  return { doc, dispatch, created, body, head, listeners }
}

// `closest` over the fake tree — the overlay's attribution walk.
function attachClosest(e: FakeEl): void {
  ;(e as unknown as { closest: (sel: string) => FakeEl | null }).closest = (
    sel: string,
  ): FakeEl | null => {
    // The overlay's only selector form is `[attr]` — match the bare name.
    const name = sel.startsWith('[') && sel.endsWith(']') ? sel.slice(1, -1) : sel
    let cur: FakeEl | undefined = e
    while (cur) {
      if (cur.attrs.has(name)) return cur
      cur = cur.parent
    }
    return null
  }
}

interface FetchCall {
  url: string
}
function fakeFetch(responses: Map<string, { status: number; body?: ComponentCostRecord }>) {
  const calls: FetchCall[] = []
  const impl = async (
    url: string,
  ): Promise<{ ok: boolean; status: number; json: () => Promise<unknown> }> => {
    calls.push({ url })
    const hit = responses.get(url)
    if (!hit) return { ok: false, status: 404, json: async () => ({ message: 'missing' }) }
    if (hit.status >= 400) return { ok: false, status: hit.status, json: async () => ({}) }
    return { ok: true, status: 200, json: async () => hit.body }
  }
  // The call log rides the function object so the asserts can dig it out.
  const fn = impl as unknown as typeof fetch & { calls: FetchCall[] }
  fn.calls = calls
  return { fn: fn as unknown as typeof fetch, calls }
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

test('mount is idempotent per document and the disposer tears everything down', () => {
  const saved = saveGlobals()
  try {
    const f = fakeDoc()
    ;(globalThis as Record<string, unknown>).document = f.doc as unknown as Document
    const first = mountCostOverlay({ doc: f.doc as unknown as Document })
    const second = mountCostOverlay({ doc: f.doc as unknown as Document })
    // A second mount is a no-op (same disposer), never a second panel or a
    // second listener set.
    assert.equal(second, first)
    const d1 = f.listeners.get('mouseover')?.length ?? 0
    const d2 = f.listeners.get('keydown')?.length ?? 0
    assert.equal(d1, 1, 'one mouseover delegation')
    assert.equal(d2, 1, 'one keydown delegation')
    first()
    assert.equal(f.listeners.get('mouseover')?.length ?? 0, 0, 'disposer unbinds mouseover')
    assert.equal(f.listeners.get('keydown')?.length ?? 0, 0, 'disposer unbinds keydown')
    // A disposed mount can be re-mounted (the guard is re-armed).
    const third = mountCostOverlay({ doc: f.doc as unknown as Document })
    assert.notEqual(third, first)
    third()
  } finally {
    restoreGlobals(saved)
  }
})

test('Alt+Shift+O arms the overlay; ?ox-cost arms it at mount', async () => {
  const saved = saveGlobals()
  try {
    const f = fakeDoc()
    const { fn } = fakeFetch(new Map())
    const page = el('main')
    const comp = el('div', { 'data-ox-src': 'src/routes/index.tsx:1' }, page)
    attachClosest(comp)
    f.body.children.push(page)
    page.parent = f.body
    const opts = {
      doc: f.doc as unknown as Document,
      fetchFn: fn,
      location: { search: '' },
    }
    const dispose = mountCostOverlay(opts)
    // No `?ox-cost` and no Alt+Shift+O yet: no panel, no fetches.
    assert.equal(f.created.filter((e) => e.tag === 'div' && e.attrs.has('data-ox-cost-panel')).length, 0)
    assert.equal(fakeCalls(f, fn).length, 0)
    // Alt+Shift+O arms it.
    f.dispatch('keydown', { altKey: true, shiftKey: true, key: 'O' })
    await flush()
    const panels = f.created.filter((e) => e.attrs.has('data-ox-cost-panel'))
    assert.equal(panels.length, 1, 'Alt+Shift+O mounts the panel')
    assert.ok(fakeCalls(f, fn).length >= 1, 'armed overlay fetches the stamped components')
    // Alt+Shift+O again disarms (the panel goes away; the mount survives).
    f.dispatch('keydown', { altKey: true, shiftKey: true, key: 'O' })
    assert.equal(panels[0].removed, true, 'second Alt+Shift+O closes the panel')
    // Escape closes too.
    f.dispatch('keydown', { altKey: true, shiftKey: true, key: 'O' })
    await flush()
    f.dispatch('keydown', { key: 'Escape' })
    assert.equal(panels[0].removed, true)
    dispose()
    void comp
  } finally {
    restoreGlobals(saved)
  }
})

test('?ox-cost at mount arms immediately and fetches the endpoint with the token', async () => {
  const saved = saveGlobals()
  try {
    const f = fakeDoc()
    const record = rec(2048, { renderMsP50: 12.34, cache: 'L0' })
    const { fn, calls } = fakeFetch(
      new Map([['/__oxide/cost?src=src%2Froutes%2Findex.tsx&token=deadbeef', { status: 200, body: record }]]),
    )
    const page = el('main')
    const comp = el('div', { 'data-ox-src': 'src/routes/index.tsx:1' }, page)
    attachClosest(comp)
    f.body.children.push(page)
    page.parent = f.body
    const dispose = mountCostOverlay({
      doc: f.doc as unknown as Document,
      fetchFn: fn,
      location: { search: '?ox-cost' },
      devToken: 'deadbeef',
    })
    await flush()
    assert.equal(calls.length, 1, 'exactly one fetch for the one stamped component')
    assert.equal(
      calls[0]?.url,
      '/__oxide/cost?src=src%2Froutes%2Findex.tsx&token=deadbeef',
      'the endpoint + the dev-token guard query',
    )
    // The panel lists the record (the three-heaviest list has it).
    const panel = f.created.find((e) => e.attrs.has('data-ox-cost-panel'))
    assert.ok(panel, 'panel exists')
    const text = (panel as unknown as { textContent?: string })?.textContent ?? ''
    assert.ok(text.includes('2.0 KB'), 'the bytes line lands in the panel')
    assert.ok(text.includes('12.3 ms'), 'the p50 line lands in the panel')
    dispose()
  } finally {
    restoreGlobals(saved)
  }
})

test('hovering a stamped component highlights it and shows its cost', async () => {
  const saved = saveGlobals()
  try {
    const f = fakeDoc()
    const record = rec(4096, { renderMsP50: null })
    const { fn } = fakeFetch(new Map([['/__oxide/cost?src=src%2Froutes%2Fblog.tsx', { status: 200, body: record }]]))
    const page = el('main')
    const comp = el('div', { 'data-ox-src': 'src/routes/blog.tsx:1' }, page)
    attachClosest(comp)
    f.body.children.push(page)
    page.parent = f.body
    const dispose = mountCostOverlay({
      doc: f.doc as unknown as Document,
      fetchFn: fn,
      location: { search: '?ox-cost' },
      devToken: '',
    })
    await flush()
    f.dispatch('mouseover', { target: comp })
    await flush()
    // The highlight rides a dedicated attribute on the component root.
    assert.equal(comp.attrs.has('data-ox-cost-hover'), true, 'hover highlights the component root')
    const panel = f.created.find((e) => e.attrs.has('data-ox-cost-panel'))
    const text = (panel as unknown as { textContent?: string })?.textContent ?? ''
    assert.ok(text.includes('4.0 KB'), 'the hovered record bytes render')
    // The spec's bytes-only note: a null renderMsP50 says so.
    assert.ok(text.includes('render time unavailable'), 'the bytes-only note renders')
    // A hover on an UNSTAMPED element keeps the previous highlight.
    const plain = el('span', {}, page)
    attachClosest(plain)
    f.dispatch('mouseover', { target: plain })
    assert.equal(comp.attrs.has('data-ox-cost-hover'), true, 'unattributed hover keeps the highlight')
    dispose()
    assert.equal(comp.attrs.has('data-ox-cost-hover'), false, 'disposer clears the highlight')
  } finally {
    restoreGlobals(saved)
  }
})

test('a 403 degrades loud (the dev-token notice), a 404 names the miss, a network error warns', async () => {
  const saved = saveGlobals()
  try {
    const f = fakeDoc()
    const warnings: string[] = []
    const win = globalThis as { console?: { warn?: (...a: unknown[]) => void } }
    const savedWarn = win.console?.warn
    win.console = { warn: (...a: unknown[]) => warnings.push(a.map(String).join(' ')) }
    const page = el('main')
    const comp = el('div', { 'data-ox-src': 'src/routes/secret.tsx:1' }, page)
    attachClosest(comp)
    f.body.children.push(page)
    page.parent = f.body
    const responses = new Map([['/__oxide/cost?src=src%2Froutes%2Fsecret.tsx', { status: 403 }]])
    const { fn } = fakeFetch(responses)
    const dispose = mountCostOverlay({
      doc: f.doc as unknown as Document,
      fetchFn: fn,
      location: { search: '?ox-cost' },
    })
    await flush()
    let panel = f.created.find((e) => e.attrs.has('data-ox-cost-panel'))
    let text = (panel as unknown as { textContent?: string })?.textContent ?? ''
    assert.ok(
      text.includes('dev token'),
      `403 names the dev token, never a fabricated record: ${text}`,
    )
    assert.equal(text.includes('4.'), false)
    dispose()
    // A 404 names the miss honestly (the endpoint's own contract).
    const missResponses = new Map([['/__oxide/cost?src=src%2Froutes%2Fsecret.tsx', { status: 404 }]])
    const miss = fakeFetch(missResponses)
    const dispose2 = mountCostOverlay({
      doc: f.doc as unknown as Document,
      fetchFn: miss.fn,
      location: { search: '?ox-cost' },
    })
    await flush()
    // `created` accumulates across mounts — the live panel is the LATEST one.
    panel = f.created.filter((e) => e.attrs.has('data-ox-cost-panel')).at(-1)
    text = (panel as unknown as { textContent?: string })?.textContent ?? ''
    assert.ok(text.includes('no cost record'), `404 names the miss: ${text}`)
    dispose2()
    // A network throw warns loud and leaves an honest empty panel.
    const throwing = async (): Promise<never> => {
      throw new Error('dial failed')
    }
    const dispose3 = mountCostOverlay({
      doc: f.doc as unknown as Document,
      fetchFn: throwing as unknown as typeof fetch,
      location: { search: '?ox-cost' },
    })
    await flush()
    assert.ok(
      warnings.some((w) => w.includes('cost')),
      `a network failure warns loud: ${warnings.join(' | ')}`,
    )
    dispose3()
    if (savedWarn) win.console.warn = savedWarn
  } finally {
    restoreGlobals(saved)
  }
})

// The fetch calls are recorded on the fake function object (see fakeFetch).
function fakeCalls(_f: ReturnType<typeof fakeDoc>, fn: unknown): FetchCall[] {
  const calls = (fn as unknown as { calls?: FetchCall[] }).calls
  return calls ?? []
}
