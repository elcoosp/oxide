// Web-vitals reporting hook (spec §16.6 / §20).
//
// Reports Core Web Vitals (LCP, CLS, INP, FCP, TTFB) from the browser to the
// server's `/_oxide/vitals` endpoint, which folds them into the Prometheus
// registry (`oxide_web_vital_count` / `_sum`). This is the *reporting hook* the
// spec names — not a bundled `web-vitals` dependency. The metrics are read off
// the browser's own `PerformanceObserver` and `performance` entries, so there
// is no third-party script in the page.
//
// Design: a single `PerformanceObserver` per metric type, kept alive for the
// page's lifetime, batching samples and flushing via `navigator.sendBeacon`
// on visibility change or after a short delay. `sendBeacon` (not `fetch`) so a
// report survives page unload — a beacon is the only reliable way to send an
// LCP/CLS sample measured during the unload window.
//
// Everything is guarded: in SSR / a non-browser runtime every function is a
// no-op, because `@oxide/react` re-exports this module and the config loader
// evaluates it in the Node worker where `window` does not exist.

/** A single reported sample. */
export interface VitalSample {
  /** `LCP` | `CLS` | `INP` | `FCP` | `TTFB` (or a custom name). */
  name: string
  /** Milliseconds for the timing metrics; unitless for CLS. */
  value: number
  /** The path the sample was measured on (`location.pathname` by default). */
  route: string
}

export interface VitalsOptions {
  /** Endpoint to POST to. Default `/_oxide/vitals`. */
  endpoint?: string
  /** Route label; default `location.pathname`. */
  route?: string
  /** Which metrics to collect. Default: all five. */
  metrics?: string[]
  /**
   * Called with each sample before it is queued. A hook for a project that
   * wants to forward vitals to its own analytics alongside the Oxide endpoint.
   * Throwing here is ignored; reporting still proceeds.
   */
  onSample?: (s: VitalSample) => void
  /** Disable the network report (e.g. a test or a privacy mode). */
  report?: boolean
}

interface MinimalPerformanceObserverEntry {
  name?: string
  value?: number
  startTime?: number
  duration?: number
  entryType?: string
  hadRecentInput?: boolean
  /** Navigation timing (`PerformanceNavigationTiming`); used for TTFB. */
  responseStart?: number
}

interface MinimalPerformanceObserver {
  observe(opts: { type: string; buffered?: boolean; durationThreshold?: number }): void
  disconnect(): void
}

type PerformanceObserverCtor = new (cb: (list: { getEntries(): MinimalPerformanceObserverEntry[] }) => void) => MinimalPerformanceObserver

interface VitalsGlobal {
  PerformanceObserver?: PerformanceObserverCtor
  performance?: { getEntriesByType(type: string): MinimalPerformanceObserverEntry[]; now(): number }
  navigator?: { sendBeacon(url: string, data?: unknown): boolean }
  document?: { visibilityState?: string; addEventListener(type: string, fn: () => void): void }
  addEventListener(type: string, fn: () => void): void
  location?: { pathname: string }
}

function g(): VitalsGlobal | undefined {
  // No cast: VitalsGlobal is entirely a subset of lib.dom's globalThis. We
  // return the globalThis reference itself and let the caller's optional
  // `w.PerformanceObserver` / `w.performance` accesses handle absence at
  // runtime. The `typeof PerformanceObserver === 'function'` guard the
  // caller uses still works (Node returns undefined, browsers return the
  // constructor). Under lib.dom types the property is non-optional, so
  // `typeof w.PerformanceObserver === 'function'` narrows correctly — the
  // runtime check catches the Node case where lib.dom's typing is wrong.
  if (typeof (globalThis as { window?: unknown }).window === 'undefined') return undefined
  return globalThis
}

/**
 * Install the reporting hook. Returns a disposer that disconnects the
 * observers and clears any pending timer. Idempotent per call — calling twice
 * installs two independent hooks (the caller owns the disposer).
 *
 * No-op (returns a no-op disposer) outside the browser.
 */
export function reportWebVitals(opts: VitalsOptions = {}): () => void {
  const w = g()
  if (!w || !w.PerformanceObserver) {
    return () => {}
  }
  const endpoint = opts.endpoint ?? '/_oxide/vitals'
  const route = opts.route ?? w.location?.pathname ?? '/'
  const want = new Set(opts.metrics ?? ['LCP', 'CLS', 'INP', 'FCP', 'TTFB'])
  const report = opts.report !== false

  const queue: VitalSample[] = []
  let timer: ReturnType<typeof setTimeout> | undefined
  let disposed = false
  const observers: MinimalPerformanceObserver[] = []

  const flush = (): void => {
    if (disposed) return
    if (timer !== undefined) {
      clearTimeout(timer)
      timer = undefined
    }
    if (!report || queue.length === 0) {
      queue.length = 0
      return
    }
    const batch = queue.splice(0, queue.length)
    try {
      // `sendBeacon` survives unload; a Blob gives the endpoint a JSON content
      // type. Fall back to `fetch(keepalive)` where sendBeacon is absent.
      const body = JSON.stringify(batch)
      if (w.navigator?.sendBeacon) {
        w.navigator.sendBeacon(endpoint, new Blob([body], { type: 'application/json' }))
      } else {
        void fetch(endpoint, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body,
          keepalive: true,
        }).catch(() => {})
      }
    } catch {
      // Reporting must never throw into page code.
    }
  }

  const push = (name: string, value: number): void => {
    if (disposed || !want.has(name)) return
    const sample: VitalSample = { name, value, route }
    try {
      opts.onSample?.(sample)
    } catch {
      // A user hook that throws must not break reporting.
    }
    queue.push(sample)
    if (timer === undefined) {
      // A short debounce: batch a burst of CLS shifts into one report.
      timer = setTimeout(flush, 1000)
    }
  }

  const observe = (type: string, handle: (e: MinimalPerformanceObserverEntry) => void): void => {
    try {
      const obs = new w.PerformanceObserver!((list) => {
        for (const e of list.getEntries()) handle(e)
      })
      obs.observe({ type, buffered: true, durationThreshold: 40 })
      observers.push(obs)
    } catch {
      // An unsupported entry type (older browser) is skipped, not fatal.
    }
  }

  // LCP: the last `largest-contentful-paint` entry before interaction.
  let lcp = 0
  observe('largest-contentful-paint', (e) => {
    if (typeof e.startTime === 'number') lcp = e.startTime
  })
  // CLS: accumulate layout-shift entries without recent input.
  let cls = 0
  observe('layout-shift', (e) => {
    if (!e.hadRecentInput && typeof e.value === 'number') cls += e.value
  })
  // INP: the worst interaction duration.
  let inp = 0
  observe('event', (e) => {
    if (typeof e.duration === 'number' && e.duration > inp) inp = e.duration
  })

  // FCP / TTFB are single paint/navigation-timing entries, read directly.
  const paintEntries = w.performance?.getEntriesByType('paint') ?? []
  for (const e of paintEntries) {
    if (e.name === 'first-contentful-paint' && typeof e.startTime === 'number') {
      push('FCP', e.startTime)
    }
  }
  const nav = w.performance?.getEntriesByType('navigation') ?? []
  if (nav[0] && typeof nav[0].responseStart === 'number') {
    push('TTFB', nav[0].responseStart)
  }

  // Flush LCP/CLS/INP when the page is being hidden — the last reliable point
  // to send a beacon.
  // The final flush. `pagehide` always means the page is going away, so flush
  // unconditionally; `visibilitychange` only flushes when it is *to* hidden.
  const flushFinal = (): void => {
    if (lcp > 0) push('LCP', lcp)
    if (cls > 0) push('CLS', cls)
    if (inp > 0) push('INP', inp)
    flush()
  }
  const onPageHide = (): void => flushFinal()
  const onVisibility = (): void => {
    if (w.document?.visibilityState === 'hidden') flushFinal()
  }
  w.addEventListener('pagehide', onPageHide)
  w.document?.addEventListener('visibilitychange', onVisibility)

  return () => {
    disposed = true
    for (const o of observers) o.disconnect()
    if (timer !== undefined) clearTimeout(timer)
  }
}
