// Interaction replay tests (spec v0.2, §V2-F03). Clicks, input and focus
// that land on a not-yet-hydrated `<ox-island>` are captured and replayed
// exactly once, in order, right after `data-ox-hydrated` becomes `done`.
//
// The fake DOM is intentionally minimal (the boot.test.ts convention): a
// parent/children chain with `closest`, capture listeners on the fake
// document, and interaction counters on the leaves. The "real DOM" check
// is exercised end-to-end by the smoke-app fixtures.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  startReplay, replayFor, resetReplay, replayPending,
  type ReplayDoc,
} from '../src/client/replay.ts'
import { hydrateIsland, type IslandHost } from '../src/client/islands.ts'

// ── Fake DOM ────────────────────────────────────────────────────────────────

interface FakeEl {
  tag: string
  attrs: Record<string, string>
  children: FakeEl[]
  parentElement: FakeEl | null
  value?: string
  checked?: boolean
  clicks: number
  focuses: number
  dispatched: Array<{ type: string; bubbles: boolean }>
}

function makeEl(tag: string, attrs: Record<string, string> = {}, children: FakeEl[] = []): FakeEl {
  const el: FakeEl = {
    tag, attrs, children, parentElement: null,
    clicks: 0, focuses: 0, dispatched: [],
  }
  for (const c of children) c.parentElement = el
  return el
}

type WithDom = FakeEl & {
  getAttribute(n: string): string | null
  setAttribute(n: string, v: string): void
  closest(sel: string): FakeEl | null
  contains(other: FakeEl): boolean
  click(): void
  focus(): void
  dispatchEvent(ev: { type: string; bubbles: boolean }): boolean
}

function domify(el: FakeEl): WithDom {
  const d = el as unknown as WithDom
  d.getAttribute = (n) => (n in el.attrs ? el.attrs[n] : null)
  d.setAttribute = (n, v) => { el.attrs[n] = v }
  d.closest = (sel) => {
    let n: FakeEl | null = el
    while (n) {
      if (sel === 'ox-island' && n.tag === 'ox-island') return n
      n = n.parentElement
    }
    return null
  }
  d.contains = (other) => {
    let n: FakeEl | null = other
    while (n) {
      if (n === el) return true
      n = n.parentElement
    }
    return false
  }
  d.click = () => { el.clicks++ }
  d.focus = () => { el.focuses++ }
  d.dispatchEvent = (ev) => { el.dispatched.push({ type: ev.type, bubbles: ev.bubbles === true }); return true }
  return d
}

function makeDoc(): ReplayDoc & {
  fire(type: string, target: WithDom): void
  captureCount(type: string): number
} {
  const listeners = new Map<string, Array<(e: { target?: unknown }) => void>>()
  return {
    addEventListener(type: string, fn: (e: { target?: unknown }) => void, _capture?: boolean) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: { target?: unknown }) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((x) => x !== fn))
    },
    querySelectorAll() {
      return []
    },
    fire(type: string, target: WithDom) {
      for (const fn of listeners.get(type) ?? []) fn({ target })
    },
    captureCount(type: string) {
      return (listeners.get(type) ?? []).length
    },
  }
}

function islandWith(button: FakeEl, attrs: Record<string, string> = {}): FakeEl {
  return makeEl('ox-island', { 'data-ox-chunk': '/chunks/i.js', ...attrs }, [button])
}

function mustRec<T>(v: T | undefined, label: string): T {
  if (v === undefined) throw new Error(`mustRec: ${label} is undefined`)
  return v
}

// ── The spec's test list ────────────────────────────────────────────────────

test('replay: click before hydrate replays exactly once (§V2-F03)', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)

  doc.fire('click', button)
  assert.equal(button.clicks, 0, 'nothing hydrated yet: no handler ran')
  assert.equal(replayPending(), 1, 'the click is captured against the pending island')

  replayFor(island)
  assert.equal(button.clicks, 1, 'replayed exactly once after hydration')
  assert.equal(replayPending(), 0, 'the record is consumed')

  replayFor(island)
  assert.equal(button.clicks, 1, 'a second replay is a no-op (no double replay)')
  stop()
})

test('replay: click after hydrate is not recorded', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button, { 'data-ox-hydrated': 'done' }))
  const stop = startReplay(doc)

  doc.fire('click', button)
  assert.equal(replayPending(), 0, 'a hydrated island owns its events: nothing captured')

  replayFor(island)
  assert.equal(button.clicks, 0, 'no replay for events the handler already saw')
  stop()
})

test('replay: hydration finishing mid-flight does not double-replay', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)

  doc.fire('click', button)
  island.attrs['data-ox-hydrated'] = 'done' // hydrate resolves between the two clicks
  doc.fire('click', button) // the handler is live now: this one must NOT be recorded
  assert.equal(replayPending(), 1, 'only the pre-hydration click is captured')

  replayFor(island)
  assert.equal(button.clicks, 1, 'exactly one replay total')
  stop()
})

test('replay: input value and event replay in order', () => {
  resetReplay()
  const doc = makeDoc()
  const input = domify(makeEl('input'))
  input.value = 'abc'
  const island = domify(islandWith(input))
  const stop = startReplay(doc)

  doc.fire('input', input)
  input.value = 'garbage' // hydration replaced the value while the island was pending
  replayFor(island)

  assert.equal(input.value, 'abc', 'value restored from the capture')
  assert.deepEqual(mustRec(input.dispatched[0], 'dispatched[0]'), { type: 'input', bubbles: true })
  assert.equal(input.dispatched.length, 1, 'the event fires exactly once')
  stop()
})

test('replay: capacity drops oldest at 100 (MAX)', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)

  for (let i = 0; i < 105; i++) doc.fire('click', button)
  assert.equal(replayPending(), 100, 'the log never exceeds MAX = 100')

  replayFor(island)
  assert.equal(button.clicks, 100, 'the newest 100 replay; the oldest 5 were dropped')
  stop()
})

test('replay: unresolvable path is dropped, not replayed on the wrong node', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)

  doc.fire('click', button)
  // Hydration / a defer swap replaced the island's content: the recorded
  // target is detached, the slot now holds a different node.
  const replacement = domify(makeEl('button'))
  island.children = [replacement]
  replacement.parentElement = island
  replayFor(island)

  assert.equal(button.clicks, 0, 'the detached target is never clicked')
  assert.equal(replacement.clicks, 0, 'the same-slot replacement is never clicked either')
  assert.equal(replayPending(), 0, 'the unresolvable record is consumed (dropped)')
  stop()
})

test('replay: template fallback content is never recorded', () => {
  resetReplay()
  const doc = makeDoc()
  // A node whose ancestor chain never reaches the island host (defer
  // fallback inside `template[data-ox-b]`): the path would be truncated
  // garbage, so capture refuses it outright.
  const fallback = domify(makeEl('button'))
  fallback.parentElement = null
  const island = domify(islandWith(makeEl('div')))
  const stop = startReplay(doc)

  doc.fire('click', fallback)
  assert.equal(replayPending(), 0, 'no record for content outside the island tree')

  replayFor(island)
  assert.equal(fallback.clicks, 0)
  stop()
})

test('replay: focus replays via focusin capture', () => {
  resetReplay()
  const doc = makeDoc()
  const input = domify(makeEl('input'))
  const island = domify(islandWith(input))
  const stop = startReplay(doc)

  doc.fire('focusin', input)
  replayFor(island)
  assert.equal(input.focuses, 1, 'focus replayed once')
  assert.equal(input.dispatched.length, 0, 'focus does not dispatch a synthetic event')
  stop()
})

test('replay: change on a checkbox restores checked (recorded delta)', () => {
  resetReplay()
  const doc = makeDoc()
  const box = domify(makeEl('input'))
  box.checked = false
  const island = domify(islandWith(box))
  const stop = startReplay(doc)

  doc.fire('change', box)
  box.checked = true // the user (or hydration) flipped it while pending
  replayFor(island)

  assert.equal(box.checked, false, 'the captured checked state is restored')
  assert.deepEqual(mustRec(box.dispatched[0], 'dispatched[0]'), { type: 'change', bubbles: true })
  stop()
})

test('replay: events outside any island are ignored', () => {
  resetReplay()
  const doc = makeDoc()
  const plain = domify(makeEl('button'))
  const island = domify(islandWith(makeEl('div')))
  const stop = startReplay(doc)

  doc.fire('click', plain)
  assert.equal(replayPending(), 0, 'no island ancestor → nothing captured')

  replayFor(island)
  assert.equal(plain.clicks, 0)
  stop()
})

test('replay: the disposer removes the capture listeners', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)
  assert.equal(doc.captureCount('click'), 1, 'capture installed on click')
  assert.equal(doc.captureCount('focusin'), 1, 'focus rides the focusin listener')

  stop()
  assert.equal(doc.captureCount('click'), 0, 'disposed')
  assert.equal(doc.captureCount('focusin'), 0)

  doc.fire('click', button)
  assert.equal(replayPending(), 0, 'nothing captured after the disposer')
  replayFor(island)
  assert.equal(button.clicks, 0)
})

test('replay: startReplay is idempotent per document (double boot safe)', () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))

  const stop1 = startReplay(doc)
  const stop2 = startReplay(doc)
  assert.equal(doc.captureCount('click'), 1, 'the second start did not double-install')

  doc.fire('click', button)
  replayFor(island)
  assert.equal(button.clicks, 1, 'one capture → one replay, even across two boots')
  stop1()
})

test('replay: startReplay without a document is an import-safe no-op', () => {
  resetReplay()
  const stop = startReplay(undefined)
  assert.equal(typeof stop, 'function', 'a no-op disposer, not a throw (SSR safety)')
  stop()
})

// ── Wiring pins: islands.ts and boot.ts ─────────────────────────────────────

test('islands: hydrateIsland replays the island’s pending interactions after done', async () => {
  resetReplay()
  const doc = makeDoc()
  const button = domify(makeEl('button'))
  const island = domify(islandWith(button))
  const stop = startReplay(doc)
  doc.fire('click', button)

  const host = island as unknown as IslandHost
  await hydrateIsland(host, {
    adapter: { hydrateIsland: () => () => {} },
    loadChunk: async () => ({}),
  })

  assert.equal(island.attrs['data-ox-hydrated'], 'done', 'hydration completed')
  assert.equal(button.clicks, 1, 'the pending click replayed as part of hydration')
  stop()
})

test('boot: interactionReplay: false skips the capture; absent enables it', async () => {
  const saved = saveGlobals()
  try {
    const enabled = await runBootWith({})
    const button = domify(makeEl('button'))
    const island = domify(islandWith(button))
    enabled.doc.fire('click', button)
    replayFor(island)
    assert.equal(button.clicks, 1, 'capture is ON by default (flag absent)')
    enabled.dispose()

    resetReplay()
    const disabled = await runBootWith({ interactionReplay: false })
    const offButton = domify(makeEl('button'))
    const offIsland = domify(islandWith(offButton))
    disabled.doc.fire('click', offButton)
    replayFor(offIsland)
    assert.equal(offButton.clicks, 0, 'capture is OFF when interactionReplay is false')
    disabled.dispose()
  } finally {
    restoreGlobals(saved)
    resetReplay()
  }
})

// ── boot.test.ts harness (minimal subset, same conventions) ─────────────────

interface Saved {
  window?: unknown
  document?: unknown
  location?: unknown
  history?: unknown
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

async function runBootWith(opts: { interactionReplay?: boolean }): Promise<{
  doc: ReturnType<typeof makeDoc>
  dispose: () => void
}> {
  // Lazy dynamic import: boot.ts is only loaded by the tests that need it.
  const { bootWithRouter } = await import('../src/client/boot.ts')
  const doc = makeDoc()
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const history = {
    state: null as unknown,
    pushState(s: unknown) { this.state = s },
    replaceState(s: unknown) { this.state = s },
  }
  const g = globalThis as Record<string, unknown>
  g.window = { __oxide: { route: '/home', data: '', interactionReplay: opts.interactionReplay }, location }
  g.document = doc
  g.location = location
  g.history = history
  g.__oxide = { route: '/home', data: '', interactionReplay: opts.interactionReplay }

  const adapter = {
    hydrate(): () => void { return () => {} },
    update(): void {},
  }
  const dispose = bootWithRouter(adapter)
  return { doc, dispose }
}
