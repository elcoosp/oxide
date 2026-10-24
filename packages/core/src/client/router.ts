// Core client router (spec §8.4, D9) — owned by the core, identical across
// frameworks, ~3 KB target. Responsibilities implemented here: link
// interception, History API, /__data route-data fetching (ODF, §9.5),
// budgeted cancelable prefetch on hover/focus/viewport, scroll restoration,
// the View Transitions hook, and adapter.update() dispatch.
//
// All DOM/global access goes through injectable params (`doc`, `history`,
// `location`, `fetch`, `scrollTo`, `IntersectionObserver`) so tests drive it
// with plain fakes — no jsdom.

import { decodeOdf } from './odf-decode.ts'
import { layoutChunksFor, type ClientManifestLike } from './route-manifest.ts'
import type { ClientInput, ClientManifest, FrameworkAdapter } from '../types.ts'

/** Minimal DOM surface the router needs (hand-rolled fakes in tests). */
/**
 * Narrow a DOM global that may be absent (Node, jsdom-less tests). Callers
 * pass the resolved value; a `TypeError` means the caller must pass one
 * explicitly. Using a helper (not a cast) keeps the type honest: after this
 * returns, TS knows the value is defined, and runtime errors surface at the
 * call site, not the first use of the value.
 */
function mustDom<T>(v: T | undefined, name: string): T {
  if (v === undefined) {
    throw new TypeError(`createRouter: no DOM environment (pass ${name} in non-browser tests)`)
  }
  return v
}

export interface RouterDoc {
  addEventListener(type: string, fn: (e: unknown) => void, opts?: AddEventListenerOptions): void
  removeEventListener(type: string, fn: (e: unknown) => void, opts?: AddEventListenerOptions): void
  querySelectorAll(sel: string): ArrayLike<RouterLink>
  startViewTransition?(cb: () => void): { finished: Promise<void> }
}

export interface RouterLink {
  getAttribute(name: string): string | null
  hasAttribute(name: string): boolean
  closest(sel: string): RouterLink | null
}

interface HistoryLike {
  state: unknown
  pushState(state: unknown, title: string, url: string): void
  replaceState(state: unknown, title: string, url: string): void
}

interface LocationLike {
  href: string
  pathname: string
  search: string
}

interface NavEvent {
  target?: { closest?: (sel: string) => RouterLink | null } | null
  button?: number
  metaKey?: boolean
  ctrlKey?: boolean
  shiftKey?: boolean
  altKey?: boolean
  preventDefault(): void
}

interface PopEvent {
  state?: unknown
}

/** Duck-type guard for a click/hover/focus event. The EventTarget contract
 *  hands the handler an `unknown`; this proves the fields the router reads
 *  exist before the handler narrows. */
function isNavEvent(e: unknown): e is NavEvent {
  // The EventTarget contract hands the listener an `unknown`; the router
  // only reads `target` (and the modifier fields, which are optional on
  // NavEvent). `preventDefault` is optional-chained at the click site —
  // a real browser event always has it; a synthetic test event may not.
  return e !== null && typeof e === 'object' && 'target' in e
}

/** Duck-type guard for popstate (`history.state` is the only field read). */
function isPopEvent(e: unknown): e is PopEvent {
  return e !== null && typeof e === 'object' && 'state' in e
}

/** Serialized history state marking oxide-managed entries. */
interface OxideHistoryState {
  __oxide: 1
  input: ClientInput
  scroll: { x: number; y: number }
}

export interface RouterOptions {
  adapter: Pick<FrameworkAdapter, 'update'>
  /** Unused today (SPA nav fetches `/__data`). Reserved for manifest-driven
   *  chunk preloading (§6.4); see WORKLOG. */
  manifest?: ClientManifestLike
  doc?: RouterDoc
  history?: HistoryLike
  location?: LocationLike
  fetch?: typeof fetch
  scrollTo?: (x: number, y: number) => void
  getScroll?: () => { x: number; y: number }
  IntersectionObserver?:
    | (new (cb: (entries: Array<{ target: unknown; isIntersecting: boolean }>) => void) => {
        observe(el: unknown): void
        disconnect(): void
      })
    | undefined
  /** Concurrent prefetch cap (§8.4 "budgeted and cancelable"). Default 5. */
  prefetchBudget?: number
  /**
   * §6.4: the URL prefix the client bundle is served from. The manifest's
   * chunk values are bare filenames; the router prefixes them with this so a
   * preload `import()` resolves. Defaults to `import.meta.url`'s directory
   * (`/client/` in the built entry), falling back to `/client/`.
   */
  clientBase?: string
  /**
   * §8.4 "optimistic navigation with loading boundaries": fired as soon as
   * `goto()` starts — before the `/__data` fetch — with a best-effort input
   * whose `route` is the currently-known path (the resolved route id is not
   * available yet; the framework resolves the loading component from the
   * manifest keyed on the URL's nearest route base). The hook is
   * **synchronous**; the caller renders a loading state and returns nothing.
   * Paired with {@link onNavigateEnd}, which fires after the data resolves
   * and just before `adapter.update` swaps the tree.
   */
  onNavigateStart?: (url: string) => void
  /** Fired after the `/__data` fetch resolves and just before `adapter.update`
   *  commits the new tree. The caller tears down whatever {@link
   *  onNavigateStart} rendered. Synchronous. */
  onNavigateEnd?: (input: ClientInput) => void
}

export interface Router {
  attach(): () => void
  dispose(): void
  goto(href: string, opts?: { replace?: boolean }): Promise<void>
  prefetch(href: string): void
  current(): ClientInput | undefined
}

export function createRouter(opts: RouterOptions): Router {
  const g = globalThis as {
    document?: RouterDoc
    history?: HistoryLike
    location?: LocationLike
    scrollTo?: (x: number, y: number) => void
    scrollX?: number
    scrollY?: number
    IntersectionObserver?: NonNullable<RouterOptions['IntersectionObserver']>
  }
  const doc = mustDom(opts.doc ?? g.document, 'doc')
  const history = mustDom(opts.history ?? g.history, 'history')
  const location = mustDom(opts.location ?? g.location, 'location')
  const doFetch = opts.fetch ?? ((...a: Parameters<typeof fetch>) => globalThis.fetch(...a))
  const scrollTo = opts.scrollTo ?? ((x: number, y: number) => g.scrollTo?.(x, y))
  const getScroll = opts.getScroll ?? (() => ({ x: g.scrollX ?? 0, y: g.scrollY ?? 0 }))
  const IO = opts.IntersectionObserver !== undefined ? opts.IntersectionObserver : g.IntersectionObserver
  const budget = opts.prefetchBudget ?? 5

  if (!doc || !history || !location) {
    throw new TypeError('createRouter: no DOM environment (pass doc/history/location in non-browser tests)')
  }

  const dataCache = new Map<string, ClientInput>()
  // BUG-15: bound the client data cache (insertion-order eviction).
  const DATA_CACHE_MAX = 64
  function dataCachePut(k: string, v: ClientInput): void {
    dataCache.set(k, v)
    if (dataCache.size > DATA_CACHE_MAX) {
      const oldest = dataCache.keys().next().value as string | undefined
      if (oldest !== undefined) dataCache.delete(oldest)
    }
  }
  const inFlight = new Map<string, AbortController>()
  let navSeq = 0
  let disposed = false
  let io: { observe(el: unknown): void; disconnect(): void } | undefined

  const hrefPath = (href: string): string => {
    try {
      const u = new URL(href, location.href)
      return u.pathname + u.search
    } catch {
      return href
    }
  }
  const hrefOf = (el: unknown): string => (el as RouterLink | undefined)?.getAttribute?.('href') ?? ''

  /** Same-origin, same-target, unmodified navigations are interceptable. */
  function shouldIntercept(a: RouterLink, e: Pick<NavEvent, 'button' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>): boolean {
    if (e.button !== undefined && e.button !== 0) return false
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return false
    if (a.hasAttribute('download')) return false
    const target = a.getAttribute('target')
    if (target && target !== '_self') return false
    const href = a.getAttribute('href')
    if (!href) return false
    if (a.hasAttribute('data-ox-link')) return true // forced interception
    try {
      return new URL(href, location.href).origin === new URL(location.href).origin
    } catch {
      return false
    }
  }

  async function fetchRouteData(path: string, signal?: AbortSignal): Promise<unknown> {
    const res = await doFetch(`/__data?route=${encodeURIComponent(path)}`, {
      signal,
      headers: { accept: 'application/oxide+odf, application/json' },
    })
    if (!res.ok) throw new Error(`router: /__data responded ${res.status} for ${path}`)
    const bytes = new Uint8Array(await res.arrayBuffer())
    return decodeOdf(bytes) // §9.5: route data arrives as ODF bytes
  }

  function normalizeInput(decoded: unknown, path: string): ClientInput {
    const d = (typeof decoded === 'object' && decoded !== null ? decoded : {}) as Record<string, unknown>
    const input: ClientInput = {
      route: (d.route as string) ?? path,
      url: (d.url as string) ?? path,
      params: (d.params as ClientInput['params']) ?? {},
      data: 'data' in d ? d.data : d,
    }
    // §7.1 / §9.2: the `/__data` envelope also carries the loader's resolved
    // `defer()` values and the request locale. Dropping them made a client
    // navigation's deferred slots hang and its locale blank — the nav data
    // diverged from what SSR had. Map them when present.
    if (d.deferred && typeof d.deferred === 'object') {
      input.deferred = d.deferred as Record<string, unknown>
    }
    if (typeof d.locale === 'string' && d.locale.length > 0) {
      input.locale = d.locale
    }
    // §8.2: the layout-id chain the new page nests inside — passed to the
    // adapter's `update` so the client re-renders `RootLayout(Slow)` matching
    // the SSR tree instead of a bare page.
    if (Array.isArray((d as { layouts?: unknown }).layouts)) {
      input.layouts = (d as { layouts: unknown[] }).layouts.filter(
        (x): x is string => typeof x === 'string',
      )
    }
    return input
  }

  // §6.4 manifest-driven chunk preloading. `opts.manifest` maps a route entry
  // id (`routes/blog/_slug_/page`) to its client chunks. We match the URL to
  // the manifest's route pattern (segment count + static/param rules) and
  // preload the winning entry's chunks so the next navigation has no JS
  // waterfall. `null` = looked up, no match; `undefined` = not yet attempted.
  const routeManifest = opts.manifest
  const manifestCache = new Map<string, string[]>()
  function chunksFor(path: string): string[] {
    if (!routeManifest) return []
    const hit = manifestCache.get(path)
    if (hit) return hit
    // `routes/index/page` is the manifest key for the root URL; normalize a
    // trailing-empty root to `index` before segment comparison.
    const segs = path.split('/').filter(Boolean)
    let best: string[] | undefined
    let bestStaticCount = -1
    // O1: capture the winning route id so the layout chain's chunks can
    // be appended after the loop. Without this a prefetch warms the page
    // but not its layouts, so the first navigation fetches the layout
    // chunk mid-render -- the latency the prefetch exists to hide.
    let matchedRouteId: string | undefined
    for (const [routeId, entry] of Object.entries(routeManifest.routes)) {
      if (routeId === '__boot__') continue
      // `routes/<...>/page` → the pattern segments; `/route` entries
      // (endpoints) are not client page chunks and are skipped.
      if (!routeId.endsWith('/page') && routeId !== 'routes/index/page') continue
      const m = /^routes\/(.*?)\/page$/.exec(routeId)
      if (!m) continue
      const routeSegs = m[1] ? m[1].split('/').filter(Boolean) : []
      // Root: `routes/index/page` → segs `['index']`, URL `/` → `[]`.
      const effectiveUrl = segs.length === 0 ? ['index'] : segs
      if (routeSegs.length !== effectiveUrl.length) continue
      let staticCount = 0
      let ok = true
      for (let i = 0; i < routeSegs.length; i++) {
        const rs = routeSegs[i]
        if (rs === undefined) { ok = false; break }
        // Param segment forms: `_slug_` (the manifest normalizes `[slug]`),
        // `[...rest]`, `[slug]`.
        const isParam = /^_|\[/.test(rs)
        if (isParam) {
          continue
        }
        if (rs !== effectiveUrl[i]) {
          ok = false
          break
        }
        staticCount++
      }
      if (!ok) continue
      if (staticCount > bestStaticCount) {
        bestStaticCount = staticCount
        best = entry.preload ?? entry.chunks ?? []
        matchedRouteId = routeId
      }
    }
    // O1: append the layout chain's chunks. Page chunks stay first so the
    // page's own entry resolves before layout hydration; the layout chunk
    // lands in the same preload call.
    const layoutChunks = matchedRouteId
      ? layoutChunksFor(
          matchedRouteId,
          routeManifest,
        )
      : []
    const result = [...(best ?? []), ...layoutChunks]
    manifestCache.set(path, result)
    return result
  }

  /**
   * The base path the client bundle is served from. The boot entry is served
   * as `/client/<boot>.js`, so `import.meta.url` is `<origin>/client/` and the
   * manifest's `entry_chunk` / `preload` values (bare filenames like
   * `routes/index/page.js`) resolve under it. Without this prefix a preload
   * `import('/routes/index/page.js')` 404s silently.
   *
   * The base is derived from `import.meta.url` in a module context; the
   * `clientBase` option remains as an escape hatch for callers that load the
   * router outside a module (it is not injected by the test suite).
   */
  const CLIENT_BASE: string = (() => {
    if (opts.clientBase !== undefined) return opts.clientBase
    try {
      // eslint-disable-next-line no-undef
      const u = (import.meta as { url?: string }).url
      if (typeof u === 'string' && u.length > 0) {
        return new URL('.', u).pathname
      }
    } catch {
      // import.meta unavailable (non-module context) — fall through.
    }
    return '/client/'
  })()

  /** Preload a route's manifest chunks via `import()`; the bundler dedupes. */
  function preloadChunks(path: string): void {
    for (const chunk of chunksFor(path)) {
      // A dynamic import warms the module cache with no execution side effects.
      // `chunk` is a manifest filename relative to the client base.
      const url = chunk.startsWith('/') ? chunk : `${CLIENT_BASE}${chunk}`
      void import(/* @vite-ignore */ url).catch(() => {
        // A missing preload chunk is not fatal — the real navigation will
        // still fetch it (or 404 loudly) via the boot entry.
      })
    }
  }

  /** Budgeted, cancelable prefetch (§8.4): max `budget` in flight. */
  function prefetch(href: string): void {
    const path = hrefPath(href)
    // §6.4: preload the route's JS chunks in parallel with the data fetch.
    // Both are silent on failure; a real `goto` retries.
    preloadChunks(path)
    if (disposed || dataCache.has(path) || inFlight.has(path) || inFlight.size >= budget) return
    const ac = new AbortController()
    inFlight.set(path, ac)
    fetchRouteData(path, ac.signal)
      .then((decoded) => {
        dataCachePut(path, normalizeInput(decoded, path))
      })
      .catch(() => {
        // prefetch failures are silent — a real goto retries over the wire
      })
      .finally(() => {
        inFlight.delete(path)
      })
  }

  async function goto(href: string, nav: { replace?: boolean } = {}): Promise<void> {
    const path = hrefPath(href)
    const navId = ++navSeq
    // §6.4: kick off chunk preload immediately; the data fetch below runs in
    // parallel. Idempotent (the bundler dedupes a warm module).
    preloadChunks(path)
    let input = dataCache.get(path)
    // §8.4: optimistic navigation with loading boundaries. `onNavigateStart`
    // fires BEFORE the fetch so the caller can render a loading state. If
    // the data is already cached, the hook is skipped — the swap is immediate.
    if (!input) {
      try {
        opts.onNavigateStart?.(path)
      } catch (e) {
        console.warn?.('router: onNavigateStart threw', e)
      }
      try {
        const decoded = await fetchRouteData(path)
        if (navId !== navSeq) return // a newer navigation superseded this one
        input = normalizeInput(decoded, path)
        dataCachePut(path, input)
      } catch (e) {
        // Navigation failed — tear down the loading state and propagate.
        try {
          opts.onNavigateEnd?.({ route: path, url: path, params: {}, data: {} })
        } catch {
          // swallow — cleanup errors must not mask the real fetch failure
        }
        throw e
      }
    }
    if (navId !== navSeq) {
      // A newer navigation took over; tear down the loading state it superseded.
      try {
        opts.onNavigateEnd?.(input)
      } catch {
        // swallow
      }
      return
    }
    // save the departure scroll into the entry we are leaving (§8.4)
    const prev = history.state as Partial<OxideHistoryState> | null
    if (prev && prev.__oxide) prev.scroll = getScroll()
    const state: OxideHistoryState = { __oxide: 1, input, scroll: { x: 0, y: 0 } }
    const commit = () => {
      // Fires just before the tree swaps. The caller tears down whatever
      // `onNavigateStart` rendered; the swap itself is `adapter.update`.
      try {
        opts.onNavigateEnd?.(input)
      } catch (e) {
        console.warn?.('router: onNavigateEnd threw', e)
      }
      history[nav.replace ? 'replaceState' : 'pushState'](state, '', path)
      opts.adapter.update(input)
    }
    // View Transitions hook (§8.4): wrap the swap when the platform has it
    if (doc.startViewTransition) doc.startViewTransition(commit)
    else commit()
  }

  function onClick(e: unknown): void {
    if (!isNavEvent(e)) return
    const a = e.target?.closest?.('a')
    if (!a || !shouldIntercept(a, e)) return
    e.preventDefault?.()
    void goto(a.getAttribute('href') as string)
  }

  function onHover(e: unknown): void {
    if (!isNavEvent(e)) return
    const a = e.target?.closest?.('a')
    if (a && shouldIntercept(a, e)) prefetch(a.getAttribute('href') as string)
  }

  function onPopstate(e: unknown): void {
    if (!isPopEvent(e)) return
    const state = e.state as Partial<OxideHistoryState> | null
    if (state && state.__oxide && state.input) {
      opts.adapter.update(state.input)
      if (state.scroll) scrollTo(state.scroll.x, state.scroll.y)
      return
    }
    // Unknown state (e.g. initial entry): fetch data for the current location.
    const path = location.pathname + location.search
    const navId = ++navSeq
    fetchRouteData(path)
      .then((decoded) => {
        if (navId === navSeq) opts.adapter.update(normalizeInput(decoded, path))
      })
      .catch(() => {})
  }

  function attach(): () => void {
    doc.addEventListener('click', onClick)
    doc.addEventListener('mouseover', onHover)
    doc.addEventListener('focusin', onHover)
    doc.addEventListener('popstate', onPopstate)
    if (IO) {
      io = new IO((entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) prefetch(hrefOf(entry.target))
        }
      })
      const links = doc.querySelectorAll('a[href]')
      for (let i = 0; i < links.length; i++) {
        const a = links[i] as RouterLink
        if (shouldIntercept(a, {})) io.observe(a)
      }
    }
    return dispose
  }

  function dispose(): void {
    disposed = true
    doc.removeEventListener('click', onClick)
    doc.removeEventListener('mouseover', onHover)
    doc.removeEventListener('focusin', onHover)
    doc.removeEventListener('popstate', onPopstate)
    io?.disconnect()
    io = undefined
    for (const ac of inFlight.values()) ac.abort()
    inFlight.clear()
  }

  return {
    attach,
    dispose,
    goto,
    prefetch,
    current: () => {
      const state = history.state as Partial<OxideHistoryState> | null
      return state?.input ?? dataCache.get(location.pathname + location.search)
    },
  }
}
