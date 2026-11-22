// §V2-R01 C42 — learned prefetch picks the dominant edge (F13).
//
// Pins `packages/core/src/client/prefetch.ts`: `pickEdges` keeps edges at or
// above `minProbability` capped at the budget (the dominant edge — highest
// probability — is first); `edgeToDataPath` inverts a param-free route id to
// its concrete `/__data` path (param edges warm chunks only); and
// `startLearnedPrefetch` warms the dominant edge's chunks + data after idle.

import { edgeToDataPath, pickEdges, startLearnedPrefetch, type NavEdge } from '../../packages/core/src/client/prefetch.ts'
import type { Assertion } from './types.ts'

const c42: Assertion = {
  id: 'C42',
  name: 'learned prefetch picks the dominant edge (F13)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    // pickEdges: filter by minProbability, cap at the budget. The dominant
    // edge (highest probability) stays first — the server sorts, pickEdges
    // preserves order, so the speculation-rules hint lands on it.
    const edges: NavEdge[] = [
      { to: 'routes/about/page', probability: 0.5 },
      { to: 'routes/blog/_slug_/page', probability: 0.2 },
      { to: 'routes/contact/page', probability: 0.4 },
    ]
    const picked = pickEdges(edges, 0.35, 3)
    if (picked.length !== 2 || picked[0]?.to !== 'routes/about/page' || picked[1]?.to !== 'routes/contact/page') {
      throw new Error(
        `pickEdges keeps ≥minProbability, dominant first: got ${JSON.stringify(picked.map((e) => e.to))}`,
      )
    }
    // The budget caps the take.
    const capped = pickEdges(edges, 0.0, 1)
    if (capped.length !== 1 || capped[0]?.to !== 'routes/about/page') {
      throw new Error(`the budget caps the take (got ${capped.length}, expected 1, dominant first)`)
    }

    // edgeToDataPath: a param-free id inverts to a concrete path; a param id
    // does not (its prefetch warms chunks only); the root id inverts to '/'.
    if (edgeToDataPath('routes/about/page') !== '/about') {
      throw new Error(`edgeToDataPath('routes/about/page') → '/about'`)
    }
    if (edgeToDataPath('routes/index/page') !== '/') {
      throw new Error(`edgeToDataPath('routes/index/page') → '/'`)
    }
    if (edgeToDataPath('routes/blog/_slug_/page') !== undefined) {
      throw new Error(`a param edge has no single concrete path (got ${edgeToDataPath('routes/blog/_slug_/page')})`)
    }

    // startLearnedPrefetch: after idle, the loop fetches /_oxide/nav, picks
    // the edges, warms the dominant edge's chunks + data. The dominant edge
    // (routes/about/page — param-free) gets a data fetch; the param edge
    // (routes/blog/_slug_/page) warms chunks only.
    const warmed: string[] = []
    const dataFetched: string[] = []
    let speculated = ''
    const manifest = {
      routes: {
        'routes/index/page': { preload: [], chunks: [], layout_chain: [] },
        'routes/about/page': { preload: ['/chunks/about.js'], chunks: [], layout_chain: [] },
        'routes/blog/_slug_/page': { preload: ['/chunks/blog.js'], chunks: [], layout_chain: [] },
      },
    }
    const pf = startLearnedPrefetch(
      {
        fetch: (async (input: string) => {
          if (typeof input === 'string' && input.startsWith('/_oxide/nav')) {
            return {
              ok: true,
              status: 200,
              json: async () => ({
                from: 'routes/index/page',
                edges: [
                  { to: 'routes/about/page', probability: 0.6 },
                  { to: 'routes/blog/_slug_/page', probability: 0.4 },
                ],
              }),
            }
          }
          return { ok: false, status: 404, json: async () => undefined }
        }) as PrefetchFetch,
        requestIdle: (cb) => cb(),
        // Defer the callback (real setTimeout runs the cb later, after
        // `const h = setTimeout(...)` assigns `h`; a synchronous fake hits the
        // TDZ inside `schedule`). queueMicrotask keeps it deterministic.
        setTimeout: (cb) => { queueMicrotask(() => cb()); return {} },
        clearTimeout: () => {},
        manifest,
        currentPath: () => '/',
        loadChunk: (chunk: string) => { warmed.push(chunk) },
        fetchData: (path: string) => { dataFetched.push(path) },
        injectSpeculationRules: (url: string) => { speculated = url },
        onCancelOnNavigate: () => {},
      },
      0.35,
    )
    // The loop schedules prefetches on timers (2s spacing); the fake setTimeout
    // fires synchronously inside the nav-fetch .then, so flushing the
    // microtask queue + one macrotask drains every picked edge's prefetch.
    await new Promise((r) => setTimeout(r, 10))
    pf.dispose()

    if (!warmed.includes('/chunks/about.js') || !warmed.includes('/chunks/blog.js')) {
      throw new Error(`the dominant + param edges warmed their chunks (warmed=${JSON.stringify(warmed)})`)
    }
    // The param-free dominant edge gets a data fetch; the param edge does not.
    if (!dataFetched.includes('/about')) {
      throw new Error(`the dominant (param-free) edge gets a /__data fetch (dataFetched=${JSON.stringify(dataFetched)})`)
    }
    if (dataFetched.some((p) => p.includes('blog'))) {
      throw new Error(`a param edge never gets a /__data fetch (dataFetched=${JSON.stringify(dataFetched)})`)
    }
    // The speculation-rules hint lands on the dominant edge's concrete URL.
    if (speculated !== '/about') {
      throw new Error(`speculation-rules hint for the dominant edge's URL (got ${speculated})`)
    }
    return { ok: true, detail: `pickEdges(dominant first) + edgeToDataPath + prefetch warmed ${warmed.length} chunks, data ${JSON.stringify(dataFetched)}` }
  },
}

// The prefetcher's fetch seam (a narrow subset of the global fetch shape).
type PrefetchFetch = (input: string, init?: { signal?: AbortSignal; headers?: Record<string, string> }) => Promise<{
  ok: boolean
  status: number
  json: () => Promise<unknown>
}>

export default c42
