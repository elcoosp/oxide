// V2-F13 — the client learned-prefetch pins. Written FIRST (TDD red)
// against the missing `packages/core/src/client/prefetch.ts` module; the
// router's navigation-header stamping is pinned in `router.test.ts`.
//
// The contract:
// - `connectionAllowsPrefetch`: Data Saver or a 2g effective type opts
//   out; an absent connection object (Firefox, tests) allows.
// - `pickEdges`: the `minProbability` threshold (the typed config's
//   `prefetch.minProbability`, spec default 0.35) + the 3-edge cap.
// - `navRouteHeaders`: both nav headers carry MANIFEST ROUTE IDS (never
//   URLs with params); `undefined` when either side fails to resolve.
// - `edgeToDataPath`: only a param-free route id inverts to exactly one
//   concrete `/__data` path; param edges prefetch chunks only.
// - `startLearnedPrefetch`: the spec's runtime — idle-gated, one new
//   prefetch per 2s, cancelled on navigation, chunk+data split per edge,
//   speculation-rules hint for the top param-free edge.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  connectionAllowsPrefetch,
  edgeToDataPath,
  navRouteHeaders,
  pickEdges,
  startLearnedPrefetch,
  type PrefetcherDeps,
} from '../src/client/prefetch.ts'
import type { ClientManifestLike } from '../src/client/route-manifest.ts'

/** The manifest shape the router tests use: two static routes, one param
 * route, a layout chain on the index. */
const MANIFEST: ClientManifestLike = {
  routes: {
    'routes/index/page': { preload: ['index.js'], layout_chain: ['routes/layout'] },
    'routes/layout': { preload: ['layout.js'] },
    'routes/about/page': { preload: ['about.js'], layout_chain: ['routes/layout'] },
    'routes/blog/_slug_/page': { preload: ['blog.js'] },
  },
}

// ------------------------------------------------------- the connection gate

test('connectionAllowsPrefetch refuses Data Saver and 2g', () => {
  assert.equal(connectionAllowsPrefetch({ saveData: true, effectiveType: '4g' }), false)
  assert.equal(connectionAllowsPrefetch({ effectiveType: '2g' }), false)
  assert.equal(connectionAllowsPrefetch({ effectiveType: 'slow-2g' }), false)
})

test('connectionAllowsPrefetch allows fast connections and absent probes', () => {
  assert.equal(connectionAllowsPrefetch({ effectiveType: '4g' }), true)
  assert.equal(connectionAllowsPrefetch({ effectiveType: '3g' }), true)
  assert.equal(connectionAllowsPrefetch(undefined), true, 'no connection object = no opt-out signal')
})

// ------------------------------------------------------------- edge picking

test('pickEdges keeps edges at or above the threshold in order', () => {
  const edges = [
    { to: 'routes/a/page', probability: 0.6 },
    { to: 'routes/b/page', probability: 0.35 },
    { to: 'routes/c/page', probability: 0.34 },
    { to: 'routes/d/page', probability: 0.1 },
  ]
  const picked = pickEdges(edges, 0.35)
  assert.deepEqual(picked.map((e) => e.to), ['routes/a/page', 'routes/b/page'])
})

test('pickEdges caps at three unless told otherwise', () => {
  const edges = [0.9, 0.8, 0.7, 0.6, 0.5].map((p, i) => ({
    to: `routes/r${i}/page`,
    probability: p,
  }))
  assert.equal(pickEdges(edges, 0.35).length, 3)
  assert.equal(pickEdges(edges, 0.35, 5).length, 5)
})

// ------------------------------------------------------- the header contract

test('navRouteHeaders carries route ids for both sides of a navigation', () => {
  const headers = navRouteHeaders('/', '/about', MANIFEST)
  assert.deepEqual(headers, {
    'x-oxide-nav-from': 'routes/index/page',
    'x-oxide-nav-to': 'routes/about/page',
  })
})

test('navRouteHeaders normalizes a param URL to the target manifest id', () => {
  // The target path is a concrete URL; the header carries the id the
  // server stores — the manifest key, not the param-carrying URL.
  const headers = navRouteHeaders('/about', '/blog/hello', MANIFEST)
  assert.equal(headers?.['x-oxide-nav-to'], 'routes/blog/_slug_/page')
  assert.ok(!JSON.stringify(headers).includes('/blog/hello'), 'no URL with params rides the wire')
})

test('navRouteHeaders refuses when either side fails to resolve', () => {
  assert.equal(navRouteHeaders('/', '/about', undefined), undefined, 'no manifest = no headers')
  assert.equal(
    navRouteHeaders('/', '/unmatched-route', MANIFEST),
    undefined,
    'an unmatched target resolves no id',
  )
})

test('navRouteHeaders refuses a self-edge', () => {
  assert.equal(navRouteHeaders('/about', '/about', MANIFEST), undefined)
})

// ------------------------------------------------------------ the id → path split

test('edgeToDataPath inverts only param-free ids', () => {
  assert.equal(edgeToDataPath('routes/about/page'), '/about')
  assert.equal(edgeToDataPath('routes/index/page'), '/')
  assert.equal(edgeToDataPath('routes/blog/_slug_/page'), undefined, 'a param id has no single path')
  assert.equal(edgeToDataPath('routes/blog/[slug]/page'), undefined, 'the label form is param-shaped too')
  assert.equal(edgeToDataPath('routes/index'), undefined, 'a non-page id never inverts')
})

// ---------------------------------------------------------------- the runtime

/** The dep set a test drives: an immediately-firing idle, a manually
 * advanced timer queue and a scripted fetch. */
function makeDeps(body: unknown, opts: { conn?: { saveData?: boolean; effectiveType?: string } } = {}) {
  const timers: { cb: () => void; ms: number }[] = []
  const chunks: string[] = []
  const data: string[] = []
  const specRules: string[] = []
  const debug: string[] = []
  let cancelHook: (() => void) | undefined
  let disposed = false
  let idleCb: (() => void) | undefined
  const deps: PrefetcherDeps = {
    fetch: (() => {
      if (disposed) return Promise.reject(new Error('disposed'))
      return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(body) })
    }) as PrefetcherDeps['fetch'],
    requestIdle: (cb) => {
      idleCb = cb
    },
    setTimeout: (cb, ms) => {
      timers.push({ cb, ms })
      return timers.length
    },
    clearTimeout: () => {},
    conn: opts.conn,
    manifest: MANIFEST,
    currentPath: () => '/',
    loadChunk: (c) => chunks.push(c),
    fetchData: (p) => data.push(p),
    injectSpeculationRules: (u) => specRules.push(u),
    onCancelOnNavigate: (cancel) => {
      cancelHook = cancel
    },
    debug: (m) => debug.push(m),
  }
  return {
    deps,
    timers,
    chunks,
    data,
    specRules,
    debug,
    advanceAll() {
      while (timers.length > 0) timers.shift()?.cb()
    },
    cancel: () => cancelHook?.(),
    runIdle() {
      idleCb?.()
    },
    dispose() {
      disposed = true
    },
  }
}

test('startLearnedPrefetch queries the nav endpoint and warms the top edges', async () => {
  const h = makeDeps({
    from: 'routes/index/page',
    edges: [
      { to: 'routes/about/page', probability: 0.8 },
      { to: 'routes/blog/_slug_/page', probability: 0.5 },
      { to: 'routes/never/page', probability: 0.1 },
    ],
  })
  startLearnedPrefetch(h.deps, 0.35)
  await Promise.resolve()
  h.runIdle()
  await new Promise((r) => setImmediate(r))
  h.advanceAll()
  // Chunks for the two qualifying edges (about + its layout chain; blog).
  assert.ok(h.chunks.includes('about.js'))
  assert.ok(h.chunks.includes('layout.js'), 'the layout chain warms with the page')
  assert.ok(h.chunks.includes('blog.js'))
  assert.ok(!h.chunks.includes('index.js'), 'the non-qualifying edge is not warmed')
  // Data only for the param-free edge.
  assert.deepEqual(h.data, ['/about'])
  // The speculation-rules hint rides the top edge (param-free).
  assert.deepEqual(h.specRules, ['/about'])
})

test('the prefetch loop respects the 2s spacing between starts', async () => {
  const h = makeDeps({
    edges: [0.9, 0.8, 0.7].map((p, i) => ({ to: `routes/r${i}/page`, probability: p })),
  })
  startLearnedPrefetch(h.deps, 0.35)
  h.runIdle()
  await new Promise((r) => setImmediate(r))
  assert.equal(h.timers.length, 3)
  assert.deepEqual(
    h.timers.map((t) => t.ms),
    [0, 2000, 4000],
    'one new prefetch per 2s',
  )
})

test('the loop skips when the connection opts out', async () => {
  const h = makeDeps({ edges: [{ to: 'routes/about/page', probability: 0.9 }] }, {
    conn: { saveData: true },
  })
  startLearnedPrefetch(h.deps, 0.35)
  h.runIdle()
  await new Promise((r) => setImmediate(r))
  assert.equal(h.timers.length, 0, 'no prefetch scheduled under Data Saver')
  assert.ok(h.debug.some((m) => m.includes('connection')), 'the refusal is visible, not silent')
})

test('a navigation cancels the in-flight nav query', async () => {
  const h = makeDeps({ edges: [{ to: 'routes/about/page', probability: 0.9 }] })
  let pending!: Promise<unknown>
  let lastSignal: AbortSignal | undefined
  h.deps.fetch = ((_input: string, init?: { signal?: AbortSignal }) => {
    lastSignal = init?.signal
    pending = new Promise(() => {})
    return pending as unknown as ReturnType<PrefetcherDeps['fetch']>
  }) as PrefetcherDeps['fetch']
  void pending
  startLearnedPrefetch(h.deps, 0.35)
  h.runIdle()
  await new Promise((r) => setImmediate(r))
  h.cancel() // the navigation hook fires aborts the controller the query rides
  assert.ok(lastSignal?.aborted, "the nav query's AbortController fired on navigation")
})

test('dispose stops the loop and clears the timers', async () => {
  const h = makeDeps({ edges: [{ to: 'routes/about/page', probability: 0.9 }] })
  const pf = startLearnedPrefetch(h.deps, 0.35)
  h.runIdle()
  await new Promise((r) => setImmediate(r))
  pf.dispose()
  h.advanceAll()
  assert.equal(h.chunks.length, 0, 'disposed: nothing warms')
})
