// Interaction-replay micro-bench (§V2-F03). Zero-dependency, Node-only
// (type stripping); NOT part of the CI test glob (`*.test.ts`) — run it
// directly: `node --experimental-strip-types test/replay.bench.ts`
// (Node ≥ 24 strips types without the flag).
//
// Reports ns/op (median of runs) for the three paths the feature adds to
// the page:
//   capture/miss    — a click OUTSIDE any island (the common case on
//                     island-free pages: one `closest` walk to the root)
//   capture/record  — a click on a pending island (closest + path walk + log)
//   replay/dispatch — replayFor over a full 100-record log (resolve + click)
// The replay acceptance contract is the F05 gauge pattern: the capture
// must stay O(depth) with no allocation beyond the record itself, and
// replay dispatch must be O(records) with no per-record allocation beyond
// the synthetic event.
import { performance } from 'node:perf_hooks'
import { startReplay, replayFor, resetReplay } from '../src/client/replay.ts'

interface FakeEl {
  tag: string
  attrs: Record<string, string>
  children: FakeEl[]
  parentElement: FakeEl | null
  clicks: number
}

function makeEl(tag: string, attrs: Record<string, string> = {}, children: FakeEl[] = []): FakeEl {
  const el: FakeEl = { tag, attrs, children, parentElement: null, clicks: 0 }
  for (const c of children) c.parentElement = el
  const d = el as FakeEl & {
    getAttribute(n: string): string | null
    closest(sel: string): FakeEl | null
    click(): void
  }
  d.getAttribute = (n) => (n in el.attrs ? el.attrs[n] : null)
  d.closest = (sel) => {
    let n: FakeEl | null = el
    while (n) {
      if (sel === 'ox-island' && n.tag === 'ox-island') return n
      n = n.parentElement
    }
    return null
  }
  d.click = () => { el.clicks++ }
  return d
}

interface BenchDoc {
  listeners: Map<string, Array<(e: { target?: unknown }) => void>>
  addEventListener(type: string, fn: (e: { target?: unknown }) => void): void
  removeEventListener(type: string, fn: (e: { target?: unknown }) => void): void
  querySelectorAll(): FakeEl[]
}

function makeDoc(): BenchDoc {
  const listeners = new Map<string, Array<(e: { target?: unknown }) => void>>()
  return {
    listeners,
    addEventListener(type: string, fn: (e: { target?: unknown }) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: { target?: unknown }) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((x) => x !== fn))
    },
    querySelectorAll() {
      return []
    },
  }
}

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  const lo = s[mid - 1]
  const hi = s[mid]
  if (lo === undefined) return hi ?? 0
  if (hi === undefined) return lo
  return s.length % 2 === 1 ? hi : (lo + hi) / 2
}

/** ns/op = median over `runs` samples of `ops` calls each. */
function bench(name: string, runs: number, ops: number, fn: () => void): void {
  for (let i = 0; i < 2_000; i++) fn() // warmup
  const samples: number[] = []
  for (let r = 0; r < runs; r++) {
    const t0 = performance.now()
    for (let i = 0; i < ops; i++) fn()
    samples.push(((performance.now() - t0) * 1e6) / ops)
  }
  const ns = median(samples)
  const label = ns >= 1000 ? `${(ns / 1000).toFixed(2)} µs` : `${ns.toFixed(1)} ns`
  console.log(`${name.padEnd(18)} ${label}/op  (median of ${runs}×${ops})`)
}

const RUNS = 15

// capture/miss — click outside any island (target 5 levels deep in the body)
{
  resetReplay()
  const doc = makeDoc()
  startReplay(doc)
  const leaf = makeEl('button')
  let node = leaf
  for (let i = 0; i < 5; i++) {
    const parent = makeEl('div')
    parent.children.push(node)
    node.parentElement = parent
    node = parent
  }
  const ev = { target: leaf as unknown }
  const listeners = doc.listeners.get('click') ?? []
  const l0 = listeners[0]
  if (!l0) throw new Error('capture listener missing')
  bench('capture/miss', RUNS, 200_000, () => l0(ev))
}

// capture/record — click on a pending island (log pinned at 100 by shifting)
{
  resetReplay()
  const doc = makeDoc()
  startReplay(doc)
  const button = makeEl('button')
  const island = makeEl('ox-island', { 'data-ox-chunk': '/chunks/i.js' }, [button])
  island.getAttribute('data-ox-chunk') // shape the fake like the test harness
  const ev = { target: button as unknown }
  const listeners = doc.listeners.get('click') ?? []
  const l0 = listeners[0]
  if (!l0) throw new Error('capture listener missing')
  let n = 0
  bench('capture/record', RUNS, 100_000, () => {
    n++
    if (n > 100) {
      // keep the log at its bound so shift() is exercised, not unbounded growth
      island.attrs['data-ox-hydrated'] = 'done'
      replayFor(island)
      island.attrs['data-ox-hydrated'] = ''
      n = 0
    }
    l0(ev)
  })
}

// replay/dispatch — full 100-record log replayed
{
  resetReplay()
  const doc = makeDoc()
  startReplay(doc)
  const button = makeEl('button')
  const island = makeEl('ox-island', { 'data-ox-chunk': '/chunks/i.js' }, [button])
  const ev = { target: button as unknown }
  const listeners = doc.listeners.get('click') ?? []
  const l0 = listeners[0]
  if (!l0) throw new Error('capture listener missing')
  bench('replay/dispatch', RUNS, 20_000, () => {
    for (let i = 0; i < 100; i++) l0(ev)
    replayFor(island) // 100 resolves + clicks, log drained
  })
}

resetReplay()
