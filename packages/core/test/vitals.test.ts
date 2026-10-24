// Web-vitals reporting hook (spec §16.6 / §20).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { reportWebVitals, type VitalSample } from '../src/client/vitals.ts'

/** Install a fake browser global set, run `fn`, then restore. */
function withFakeBrowser(
  opts: {
    observers?: Array<{ type: string; entries: Array<Record<string, unknown>> }>
    paint?: Array<Record<string, unknown>>
    nav?: Array<Record<string, unknown>>
    sendBeacon?: (url: string, data?: unknown) => boolean
    visibility?: string
  },
  fn: (captured: { beacons: Array<{ url: string; data: unknown }>; handlers: Record<string, Array<() => void>> }) => void,
) {
  const g = globalThis as Record<string, unknown>
  const saved = { window: g.window, PerformanceObserver: g.PerformanceObserver, performance: g.performance, navigator: g.navigator, document: g.document, addEventListener: g.addEventListener }
  const beacons: Array<{ url: string; data: unknown }> = []
  const handlers: Record<string, Array<() => void>> = {}

  // A fake PerformanceObserver: on observe(), synchronously delivers the
  // configured entries for that type to the callback.
  class FakeObserver {
    cb: (list: { getEntries(): Array<Record<string, unknown>> }) => void
    constructor(cb: FakeObserver['cb']) { this.cb = cb }
    observe(o: { type: string }) {
      const spec = (opts.observers ?? []).find((x) => x.type === o.type)
      if (spec) this.cb({ getEntries: () => spec.entries })
    }
    disconnect() {}
  }

  const set = (k: string, v: unknown) =>
    Object.defineProperty(globalThis, k, { value: v, writable: true, configurable: true })

  set('window', {})
  set('PerformanceObserver', FakeObserver)
  set('performance', {
    getEntriesByType: (t: string) => (t === 'paint' ? opts.paint ?? [] : t === 'navigation' ? opts.nav ?? [] : []),
    now: () => 0,
  })
  set('navigator', { sendBeacon: opts.sendBeacon ?? ((url: string, data: unknown) => { beacons.push({ url, data }); return true }) })
  set('document', {
    visibilityState: opts.visibility ?? 'visible',
    addEventListener: (type: string, fn: () => void) => { (handlers[type] ??= []).push(fn) },
  })
  set('addEventListener', (type: string, fn: () => void) => { (handlers[type] ??= []).push(fn) })

  try {
    fn({ beacons, handlers })
  } finally {
    for (const [k, v] of Object.entries(saved)) {
      if (v === undefined) { try { delete (globalThis as Record<string, unknown>)[k] } catch { /* ignore */ } }
      else Object.defineProperty(globalThis, k, { value: v, writable: true, configurable: true })
    }
  }
}

test('reportWebVitals is a no-op outside the browser (no window)', () => {
  // No fake browser installed → globalThis.window is undefined.
  const dispose = reportWebVitals()
  assert.equal(typeof dispose, 'function')
  dispose() // must not throw
})

test('records FCP and TTFB from performance entries', () => {
  withFakeBrowser(
    {
      paint: [{ name: 'first-contentful-paint', startTime: 123.4 }],
      nav: [{ responseStart: 42 }],
    },
    ({ beacons }) => {
      const seen: VitalSample[] = []
      const dispose = reportWebVitals({ onSample: (s) => seen.push(s), report: false })
      dispose()
      assert.ok(seen.some((s) => s.name === 'FCP' && s.value === 123.4), JSON.stringify(seen))
      assert.ok(seen.some((s) => s.name === 'TTFB' && s.value === 42), JSON.stringify(seen))
      // report:false suppresses the network send.
      assert.equal(beacons.length, 0)
    },
  )
})

test('flushes LCP/CLS/INP via sendBeacon on pagehide', () => {
  withFakeBrowser(
    {
      observers: [
        { type: 'largest-contentful-paint', entries: [{ startTime: 200 }] },
        { type: 'layout-shift', entries: [{ value: 0.05, hadRecentInput: false }, { value: 0.03, hadRecentInput: false }] },
        { type: 'event', entries: [{ duration: 90 }] },
      ],
    },
    ({ beacons, handlers }) => {
      const seen: VitalSample[] = []
      const dispose = reportWebVitals({ onSample: (s) => seen.push(s) })
      // Fire the pagehide handler.
      for (const fn of handlers['pagehide'] ?? []) fn()
      dispose()
      assert.ok(beacons.length >= 1, 'a beacon was sent')
    },
  )
})

test('CLS accumulates only shifts without recent input', () => {
  withFakeBrowser(
    {
      observers: [
        { type: 'layout-shift', entries: [{ value: 0.05, hadRecentInput: false }, { value: 0.10, hadRecentInput: true }] },
      ],
    },
    ({ handlers }) => {
      const seen: VitalSample[] = []
      const dispose = reportWebVitals({ onSample: (s) => seen.push(s), report: false })
      for (const fn of handlers['pagehide'] ?? []) fn()
      dispose()
      const cls = seen.find((s) => s.name === 'CLS')
      assert.ok(cls, JSON.stringify(seen))
      assert.ok(Math.abs(cls!.value - 0.05) < 1e-9, `CLS should be 0.05, got ${cls!.value}`)
    },
  )
})

test('a custom metric allow-list filters collection', () => {
  withFakeBrowser(
    { paint: [{ name: 'first-contentful-paint', startTime: 10 }], nav: [{ responseStart: 5 }] },
    () => {
      const seen: VitalSample[] = []
      const dispose = reportWebVitals({ metrics: ['FCP'], onSample: (s) => seen.push(s), report: false })
      dispose()
      assert.ok(seen.every((s) => s.name === 'FCP'), JSON.stringify(seen))
    },
  )
})
