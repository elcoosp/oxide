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
import type { ClientInput, ClientManifest, FrameworkAdapter } from '../types.ts'

/** Minimal DOM surface the router needs (hand-rolled fakes in tests). */
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

/** Serialized history state marking oxide-managed entries. */
interface OxideHistoryState {
  __oxide: 1
  input: ClientInput
  scroll: { x: number; y: number }
}

export interface RouterOptions {
  adapter: Pick<FrameworkAdapter, 'update'>
  manifest: ClientManifest
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
  const doc = opts.doc ?? g.document
  const history = opts.history ?? g.history
  const location = opts.location ?? g.location
  const doFetch = opts.fetch ?? ((...a: Parameters<typeof fetch>) => globalThis.fetch(...a))
  const scrollTo = opts.scrollTo ?? ((x: number, y: number) => g.scrollTo?.(x, y))
  const getScroll = opts.getScroll ?? (() => ({ x: g.scrollX ?? 0, y: g.scrollY ?? 0 }))
  const IO = opts.IntersectionObserver !== undefined ? opts.IntersectionObserver : g.IntersectionObserver
  const budget = opts.prefetchBudget ?? 5

  if (!doc || !history || !location) {
    throw new TypeError('createRouter: no DOM environment (pass doc/history/location in non-browser tests)')
  }

  const dataCache = new Map<string, ClientInput>()
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
    return {
      route: (d.route as string) ?? path,
      url: (d.url as string) ?? path,
      params: (d.params as ClientInput['params']) ?? {},
      data: 'data' in d ? d.data : d,
    }
  }

  /** Budgeted, cancelable prefetch (§8.4): max `budget` in flight. */
  function prefetch(href: string): void {
    const path = hrefPath(href)
    if (disposed || dataCache.has(path) || inFlight.has(path) || inFlight.size >= budget) return
    const ac = new AbortController()
    inFlight.set(path, ac)
    fetchRouteData(path, ac.signal)
      .then((decoded) => {
        dataCache.set(path, normalizeInput(decoded, path))
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
    let input = dataCache.get(path)
    if (!input) {
      const decoded = await fetchRouteData(path)
      if (navId !== navSeq) return // a newer navigation superseded this one
      input = normalizeInput(decoded, path)
      dataCache.set(path, input)
    }
    if (navId !== navSeq) return
    // save the departure scroll into the entry we are leaving (§8.4)
    const prev = history.state as Partial<OxideHistoryState> | null
    if (prev && prev.__oxide) prev.scroll = getScroll()
    const state: OxideHistoryState = { __oxide: 1, input, scroll: { x: 0, y: 0 } }
    const commit = () => {
      history[nav.replace ? 'replaceState' : 'pushState'](state, '', path)
      opts.adapter.update(input!)
    }
    // View Transitions hook (§8.4): wrap the swap when the platform has it
    if (doc.startViewTransition) doc.startViewTransition(commit)
    else commit()
  }

  function onClick(e: NavEvent): void {
    const a = e.target?.closest?.('a')
    if (!a || !shouldIntercept(a, e)) return
    e.preventDefault()
    void goto(a.getAttribute('href') as string)
  }

  function onHover(e: NavEvent): void {
    const a = e.target?.closest?.('a')
    if (a && shouldIntercept(a, e)) prefetch(a.getAttribute('href') as string)
  }

  function onPopstate(e: PopEvent): void {
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
