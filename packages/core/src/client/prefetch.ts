// V2-F13 — the client half of the learned prefetcher: consult the
// server's aggregate route-transition graph (`GET /_oxide/nav`, served
// when the typed config's `prefetch.learned` is on) after idle, and
// prefetch what the top edges name — the route's client chunks for every
// edge (the manifest maps route ids to chunks directly), plus the route's
// data (`/__data`) when the edge's route id is param-free and therefore
// invertible to a concrete path.
//
// Privacy posture (the spec's, kept verbatim): the graph holds aggregate
// edge counts between route ids, never URLs with params and never user
// identifiers; the k-anonymity floor (20 observations, server-side) keeps
// a transition invisible until enough sessions took it. The data-plane
// fetches the prefetcher issues carry NO nav headers — they are not
// navigations and must not pollute the graph they read.
//
// Design for testability: every effect (fetch, idle scheduling, timers,
// the connection probe, the speculation-rules injection) is injectable;
// the pure helpers (`connectionAllowsPrefetch`, `pickEdges`,
// `navRouteHeaders`, `edgeToDataPath`) are pinned directly.

import { routeIdFor, isParamSegment, type ClientManifestLike } from './route-manifest.ts'

/** The bootstrap field the render crate advertises (§V2-F13, seventh
 * `window.__oxide` field). */
export const PREFETCH_BOOTSTRAP_KEY = 'prefetch'

/** The endpoint the server answers when `prefetch.learned` is on. */
export const NAV_ENDPOINT = '/_oxide/nav'

/** The two route-id headers the server's data plane observes (§V2-F13
 * step 1, plus the to-header delta recorded in docs/deviations.md: edges
 * are route ids, and the `/__data?route=` value is a path, so the target's
 * id rides its own header). */
export const NAV_FROM_HEADER = 'x-oxide-nav-from'
export const NAV_TO_HEADER = 'x-oxide-nav-to'

/** The `window.__oxide` prefetch advertisement, `true` when the server's
 * nav endpoint is live. */
export function prefetchAdvertised(): boolean {
  return (globalThis as { __oxide?: { prefetch?: boolean } }).__oxide?.prefetch === true
}

/** The `navigator.connection`-shaped probe the spec gates on. */
export interface ConnectionLike {
  saveData?: boolean
  effectiveType?: string
}

/** `false` when the user's connection says don't: Data Saver on, or a
 * `2g`/`slow-2g` effective type. Absent connection objects (Firefox,
 * tests) prefetch — the spec's gates are opt-out signals. */
export function connectionAllowsPrefetch(conn?: ConnectionLike): boolean {
  if (!conn) return true
  if (conn.saveData === true) return false
  const t = conn.effectiveType
  if (t === '2g' || t === 'slow-2g') return false
  return true
}

/** One edge the endpoint answers: target route id + probability. */
export interface NavEdge {
  to: string
  probability: number
}

/** The `/_oxide/nav` body the server answers with. */
export interface NavResponse {
  from?: string
  edges?: NavEdge[]
}

/** Keep only the edges the client should act on: probability at or above
 * the threshold (the typed config's `prefetch.minProbability`, spec default
 * 0.35), capped at `limit` (the prefetch budget). */
export function pickEdges(edges: NavEdge[], minProbability: number, limit = 3): NavEdge[] {
  return edges
    .filter((e) => e.probability >= minProbability)
    .slice(0, limit)
}

/**
 * The two route-id headers a NAVIGATION's `/__data` fetch carries, or
 * `undefined` when either side fails to resolve against the manifest (no
 * manifest, an unmatched route — the server observes nothing).
 *
 * Route ids are the manifest's canonical keys (`routes/blog/_slug_/page`),
 * which is what the server stores and what the chunk lookup consumes —
 * never the param-carrying URL (the spec's privacy constraint).
 */
export function navRouteHeaders(
  currentPath: string,
  targetPath: string,
  manifest?: ClientManifestLike,
): Record<string, string> | undefined {
  if (!manifest) return undefined
  const from = routeIdFor(currentPath, manifest)
  const to = routeIdFor(targetPath, manifest)
  if (!from || !to || from === to) return undefined
  return { [NAV_FROM_HEADER]: from, [NAV_TO_HEADER]: to }
}

/**
 * The concrete `/__data` path for an edge's route id — only when the id
 * is param-free (every segment literal), so the inversion is exact:
 * `routes/about/page` → `/about`, `routes/index/page` → `/`. A param edge
 * (`routes/blog/_slug_/page`) has no single concrete path to fetch; its
 * prefetch warms chunks only.
 */
export function edgeToDataPath(routeId: string): string | undefined {
  const m = /^routes\/(.+)\/page$/.exec(routeId)
  const body = m?.[1]
  if (body === undefined) return undefined
  const segs = body.split('/').filter((s) => s.length > 0)
  // The root page's id is `routes/index/page`; its URL is `/`.
  if (segs.length === 1 && segs[0] === 'index') return '/'
  if (segs.length === 0 || segs.some(isParamSegment)) return undefined
  return `/${segs.join('/')}`
}

/** The chunks one edge should warm: the route's entry + preload list, plus
 * its layout chain (the same shapes `chunksFor` matches by path). */
export function edgeChunks(routeId: string, manifest: ClientManifestLike): string[] {
  const entry = manifest.routes[routeId]
  if (!entry) return []
  const page = entry.preload ?? entry.chunks ?? []
  // The layout chain's chunk list (§8.2/O1): same helper the router's
  // path-driven prefetch uses.
  const layouts = layoutChainChunks(routeId, manifest)
  return [...page, ...layouts]
}

/** The layout-chain chunk names for a route id (the `layoutChunksFor`
 * behavior, re-declared locally to keep this module's import surface
 * exactly the two route-manifest exports it needs). */
function layoutChainChunks(routeId: string, manifest: ClientManifestLike): string[] {
  const entry = manifest.routes[routeId]
  const chain = entry?.layout_chain
  if (!Array.isArray(chain)) return []
  const out: string[] = []
  for (const id of chain) {
    const layoutEntry = manifest.routes[id]
    if (!layoutEntry) continue
    const chunks = layoutEntry.preload ?? layoutEntry.chunks ?? []
    out.push(...chunks)
  }
  return out
}

/** The injectable effects the prefetcher needs (tests stub all of them). */
export interface PrefetcherDeps {
  fetch: (input: string, init?: { signal?: AbortSignal; headers?: Record<string, string> }) => Promise<{
    ok: boolean
    status: number
    json: () => Promise<unknown>
  }>
  requestIdle: (cb: () => void) => void
  setTimeout: (cb: () => void, ms: number) => unknown
  clearTimeout: (handle: unknown) => void
  /** The connection probe (defaults to `navigator.connection`). */
  conn?: ConnectionLike
  /** The route manifest (chunk lookups + id resolution). */
  manifest: ClientManifestLike
  /** The current URL path (`location.pathname` in the browser). */
  currentPath: () => string
  /** Warm one chunk URL (the boot runtime's loader; `import()` in the
   * browser). Failures are silent — a cold chunk is the pre-F13 state. */
  loadChunk?: (chunk: string) => void
  /** Fetch one route's data (`/__data`); failures are silent. */
  fetchData?: (path: string) => void
  /** Inject a `<script type="speculationrules">` for the top edge (the
   * browser does this; tests record). */
  injectSpeculationRules?: (url: string) => void
  /** Called once per navigation so the prefetcher can cancel in-flight
   * work (the spec's "cancelled on navigation"). */
  onCancelOnNavigate: (cancel: () => void) => void
  /** Log a refusal reason (the diagnostics contract: visible, not
   * silent). Tests assert on it. */
  debug?: (msg: string) => void
}

/** The prefetcher handle: `dispose` cancels everything (boot teardown). */
export interface LearnedPrefetcher {
  dispose(): void
}

/** Start the learned prefetch loop (§V2-F13 step 4):
 *
 * 1. after idle, when the connection allows it and the bootstrap
 *    advertised `prefetch:true`;
 * 2. fetch `/_oxide/nav?from=<current route id>`;
 * 3. pick the edges at or above `minProbability` (max 3);
 * 4. warm each edge's chunks; data-fetch the param-free ones; inject the
 *    speculation-rules hint for the top edge on Chromium;
 * 5. one new prefetch per 2s, max 3 outstanding, everything cancelled on
 *    a navigation.
 */
export function startLearnedPrefetch(deps: PrefetcherDeps, minProbability = 0.35): LearnedPrefetcher {
  const timers: unknown[] = []
  const controllers = new Set<AbortController>()
  let disposed = false

  const abortAll = (): void => {
    for (const ac of controllers) ac.abort()
    controllers.clear()
  }
  deps.onCancelOnNavigate(abortAll)

  function track(ac: AbortController): void {
    controllers.add(ac)
  }

  function schedule(delayMs: number, cb: () => void): void {
    const h = deps.setTimeout(() => {
      const i = timers.indexOf(h)
      if (i >= 0) timers.splice(i, 1)
      cb()
    }, delayMs)
    timers.push(h)
  }

  function prefetchEdge(edge: NavEdge): void {
    if (disposed) return
    // Chunks: every edge is id-addressable through the manifest.
    for (const chunk of edgeChunks(edge.to, deps.manifest)) deps.loadChunk?.(chunk)
    // Data: only when the id inverts to exactly one concrete path.
    const dataPath = edgeToDataPath(edge.to)
    if (dataPath !== undefined) deps.fetchData?.(dataPath)
  }

  function tick(): void {
    if (disposed) return
    if (!connectionAllowsPrefetch(deps.conn)) {
      deps.debug?.('learned prefetch skipped: the connection opts out')
      return
    }
    const from = routeIdFor(deps.currentPath(), deps.manifest)
    if (!from) {
      deps.debug?.('learned prefetch skipped: the current route has no manifest id')
      return
    }
    const ac = new AbortController()
    track(ac)
    deps
      .fetch(`${NAV_ENDPOINT}?from=${encodeURIComponent(from)}`, { signal: ac.signal })
      .then((res) => (res.ok ? res.json() : undefined))
      .then((body) => {
        if (disposed || !body) return
        const edges = (body as NavResponse).edges ?? []
        const picked = pickEdges(edges, minProbability)
        if (picked.length === 0) return
        // The spec's spacing: one new prefetch per 2s, max 3 outstanding
        // (the cap is the pick limit; the spacing serializes the starts).
        for (const [i, edge] of picked.entries()) {
          schedule(i * 2000, () => prefetchEdge(edge))
        }
        // Chromium's own hint for the single most likely next route — only
        // when the edge resolves to a concrete URL (speculation rules
        // prefetch a URL, not a chunk list).
        const top = picked[0]
        if (top) {
          const url = edgeToDataPath(top.to)
          if (url !== undefined) deps.injectSpeculationRules?.(url)
        }
      })
      .catch(() => {
        // A failed nav query is silent: the pre-F13 behavior is no
        // prefetch at all.
      })
  }

  // The spec's idle trigger: the loop kicks off once per page load (the
  // graph is server-aggregated; re-polling adds nothing within a visit).
  deps.requestIdle(() => {
    if (!disposed) tick()
  })

  return {
    dispose(): void {
      disposed = true
      abortAll()
      for (const h of timers) deps.clearTimeout(h)
      timers.length = 0
    },
  }
}
