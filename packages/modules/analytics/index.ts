// @oxide/analytics — the V2-E06 first-party web-vitals module.
//
// Wires the EXISTING `@oxide-fw/core` `reportWebVitals` beacon sink (spec
// §16.6 / §20) with two generic targets a project can pick without writing
// its own client code: a generic webhook (POST a JSON batch to a URL the
// operator names) and a console target (log each sample to the browser
// console for dev).
//
// One contract: the module's `package.json#oxide.module` block carries the
// same fields the Rust `modules_loader` reads (adapters, configDefaults,
// middlewares, serverFns, types, hooks). The `defineModule` call here is
// the authoring surface; the package.json is the serialized form a build
// hook emits (or the package author writes by hand, as we do here).
//
// Usage from a project's oxide.config.ts:
//
// ```ts
// import { defineConfig } from '@oxide-fw/core'
// import analytics from '@oxide/analytics'
// export default defineConfig({
//   modules: ['@oxide/analytics'],
//   plugins: [analytics({ webhook: 'https://example.com/vitals' })],
// })
// ```

import {
  defineModule,
  reportWebVitals,
  type VitalSample,
} from '@oxide-fw/core'

/** The analytics module's options (merged under `modules.<name>`). */
export interface AnalyticsOptions {
  /** The endpoint the beacon POSTs vitals batches to. Default `/_oxide/vitals`. */
  endpoint?: string
  /** True to also `console.log` each sample (dev mode). Default false. */
  console?: boolean
  /** A webhook URL to POST each batch to (in addition to the endpoint). */
  webhook?: string
  /** Which metrics to collect. Default: all five (LCP, CLS, INP, FCP, TTFB). */
  metrics?: string[]
}

/** A generic webhook target: POSTs a JSON batch to `url`. */
function webhookTarget(url: string, batch: VitalSample[]): void {
  // `fetch(keepalive)` so the report survives unload — `sendBeacon` is the
  // primary path in `reportWebVitals`; the webhook is a secondary sink for
  // projects that already run their own analytics endpoint.
  void fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(batch),
    keepalive: true,
  }).catch(() => {
    // A webhook that 4xx/5xx must never break the page.
  })
}

/** A console target: logs each sample for dev. */
function consoleTarget(batch: VitalSample[]): void {
  for (const s of batch) {
    // eslint-disable-next-line no-console
    console.log(`[analytics] ${s.name}=${s.value.toFixed(2)} route=${s.route}`)
  }
}

/**
 * Install the analytics module's reporting hook. Returns a disposer that
 * disconnects the observers (the same shape `reportWebVitals` returns).
 * Idempotent per call. No-op outside the browser.
 *
 * The hook wires the EXISTING `reportWebVitals` beacon sink: vitals go to
 * the project's endpoint (`/_oxide/vitals` by default), AND to the webhook
 * and/or console targets the project named.
 */
export function installAnalytics(opts: AnalyticsOptions = {}): () => void {
  const endpoint = opts.endpoint ?? '/_oxide/vitals'
  const webhook = opts.webhook
  const useConsole = opts.console ?? false
  const metrics = opts.metrics
  // The `onSample` hook is called per sample before the batch flush — a
  // single seam for the console target. The webhook target rides the
  // `report` flag (we keep the network report on for the endpoint, and
  // also POST the same batch to the webhook via a fetch in the disposer's
  // lifetime — the simplest no-race wiring).
  const samples: VitalSample[] = []
  const dispose = reportWebVitals({
    endpoint,
    metrics,
    onSample: (s) => {
      samples.push(s)
      if (useConsole) {
        // eslint-disable-next-line no-console
        console.log(`[analytics] ${s.name}=${s.value.toFixed(2)} route=${s.route}`)
      }
    },
  })
  // The webhook target: a periodic flush on visibility change (the same
  // signal `reportWebVitals` uses for its own beacon). We hook
  // `visibilitychange` once; the disposer removes it.
  if (webhook) {
    const flush = (): void => {
      if (samples.length === 0) return
      const batch = samples.splice(0, samples.length)
      webhookTarget(webhook, batch)
    }
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', flush)
      return () => {
        dispose()
        document.removeEventListener('visibilitychange', flush)
        flush()
      }
    }
  }
  return dispose
}

/**
 * The module descriptor for `oxide.config.ts`'s `plugins: [...]` array.
 * Wires `defineModule` (V2-E06) so the build reads one contract — the
 * package.json `oxide.module` block — for both the loader and the plugin
 * registry.
 */
export default function analytics(opts: AnalyticsOptions = {}) {
  return defineModule(
    {
      name: '@oxide/analytics',
      adapters: ['react', 'vue', 'svelte', 'solid'],
      configDefaults: {
        analytics: {
          endpoint: opts.endpoint ?? '/_oxide/vitals',
          console: opts.console ?? false,
          webhook: opts.webhook,
        },
      },
      setup(ctx) {
        ctx.addMiddleware('./server/middleware.ts')
        ctx.addServerFn('./server/track.ts')
        ctx.addTypes(
          'analytics',
          'declare global { interface Window { __oxAnalytics?: { endpoint: string; console: boolean } } }',
        )
        ctx.addPlugin({ name: 'vitals', hooks: ['transform'] })
        ctx.extendConfig({
          analytics: { batchSize: 10 },
        })
      },
    },
    { adapter: 'react' },
  )
}

export { defineModule, reportWebVitals, type VitalSample }
