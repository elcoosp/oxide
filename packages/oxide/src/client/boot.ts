// Oxide client boot (§10.3). The generated per-project boot entry
// (`.oxide/island-boot.mjs`, written by `oxide build`) imports this module,
// registers the framework adapter, and then calls `boot()`.
//
// This module MUST be import-safe in the SSR / config-loader environment:
// `@oxide/react` re-exports it, and `oxide.config.ts` imports `@oxide/react`
// for its adapter factory — so this file gets evaluated in the Node worker
// where `window` and `document` do not exist. Everything touching the DOM
// is behind a runtime guard; the module has no top-level side effects.

import { hydrateIslands, type IslandsOptions } from './islands.ts'
import { createRouter } from './router.ts'
import { decodeOdf } from './odf-decode.ts'
import { loadScripts } from './scripts.ts'
import type { FrameworkAdapter } from '../types.ts'

interface BootState {
  adapter?: FrameworkAdapter
  loadChunk: (chunk: string) => Promise<unknown>
}

interface BootWindow extends Window {
  __oxideBoot?: BootState
  __oxideRegisterAdapter?: (adapter: FrameworkAdapter, loadChunk: BootState['loadChunk']) => void
  __oxideBootRun?: () => void
}

/** A `Window` shaped object, or `undefined` in a non-browser environment. */
function maybeWindow(): BootWindow | undefined {
  const g = globalThis as { window?: BootWindow }
  return g.window
}

/**
 * Install the boot runtime on `window` if it isn't already there. Idempotent
 * across duplicate module evaluations (bundler dedup failure, SPA reimport).
 * No-op outside the browser.
 */
function ensureInstalled(w: BootWindow): void {
  if (w.__oxideRegisterAdapter) return
  w.__oxideBoot = {
    loadChunk: async (chunk) => {
      return await import(/* @vite-ignore */ chunk)
    },
  }
  w.__oxideRegisterAdapter = (adapter, loadChunk) => {
    const state = w.__oxideBoot
    if (!state) return
    state.adapter = adapter
    state.loadChunk = loadChunk
  }
  w.__oxideBootRun = () => {
    const state = w.__oxideBoot
    if (!state?.adapter) return
    const doc = (globalThis as { document?: Document }).document
    if (!doc) return
    const opts: IslandsOptions = {
      doc: doc as unknown as IslandsOptions['doc'],
      adapter: state.adapter,
      loadChunk: state.loadChunk,
    }
    hydrateIslands(opts)
    // §16.6: schedule server-rendered `<Script>` placeholders by strategy.
    loadScripts({ doc: doc as never })
  }
}

/**
 * Register an adapter and its chunk loader, then run the island scan. The
 * generated boot entry calls this after its static imports have resolved.
 * Safe to call multiple times: the scan is idempotent because each
 * `<ox-island>` host carries a `data-ox-hydrated` guard after its first
 * hydration attempt.
 *
 * Runs synchronously if the document is already interactive; otherwise
 * waits for `DOMContentLoaded`.
 */
export function boot(): void {
  const w = maybeWindow()
  if (!w) return
  ensureInstalled(w)
  const run = w.__oxideBootRun
  if (typeof run !== 'function') return
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return
  if (doc.readyState === 'loading') {
    doc.addEventListener('DOMContentLoaded', run, { once: true })
  } else {
    run()
  }
}

/**
 * Boot the client router (§8.4): hydrate the SSR root from `window.__oxide`,
 * then attach `createRouter` so same-origin link clicks navigate via
 * `/__data` instead of a full page load.
 *
 * Guarded and idempotent:
 * - no `document` / no adapter / no `#root` / no `window.__oxide` → no-op;
 * - a hydration failure logs and leaves the SSR markup untouched;
 * - `data` in `window.__oxide` is base64 ODF, decoded via `decodeOdf`; a
 *   decode failure falls back to the raw string rather than throwing.
 *
 * Returns a disposer (the router's `dispose`), or a no-op.
 */
export function bootWithRouter(
  adapter: FrameworkAdapter,
  opts: {
    doc?: Document
    /**
     * §8.4 loading boundaries: URL-prefix → loading component map. Keys are
     * URL prefixes (`/`, `/blog`, `/blog/x`), values are the module the
     * boot entry statically imported from `loading.tsx`. On a client
     * navigation, the longest matching prefix wins and `adapter.renderLoading`
     * paints it into an overlay until the tree swaps. Absent (or a framework
     * without `renderLoading`) → no loading boundary; the router still fires
     * the hooks, they are just no-ops here.
     */
    loading?: Record<string, unknown>
  } = {},
): () => void {
  const doc = opts.doc ?? (globalThis as { document?: Document }).document
  if (!doc) return () => {}
  const w = globalThis as {
    __oxide?: { route?: string; data?: string }
    __oxideBoot?: BootState
    location?: { pathname: string; search: string }
  }

  // (a) Root hydration — REQUIRED before the router can do anything useful.
  // On a click the router calls `adapter.update(input)`, which on the client
  // side resolves to the module-level `active?.update(input)` in
  // `packages/react/src/hydrate.ts` — and `active` is only set by
  // `hydrate()` or `mount()`. Without this step SPA navigation changes the
  // URL but leaves the DOM untouched (worse than no router at all).
  //
  // Root hydration does NOT rewrite `#root.innerHTML` when the SSR markup is
  // already present (`createClientRoot` only renders when the root is empty
  // and otherwise just binds handlers), so `<ox-island>` hosts survive and
  // the island scan below still finds them.
  let hydratedRoot = false
  const root = doc.getElementById?.('root')
  const payload = w.__oxide
  if (root && payload) {
    let data: unknown = {}
    if (typeof payload.data === 'string' && payload.data.length > 0) {
      try {
        const bin = atob(payload.data)
        const bytes = new Uint8Array(bin.length)
        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
        const decoded = decodeOdf(bytes) as Record<string, unknown>
        data = decoded && typeof decoded === 'object' && 'data' in decoded ? decoded.data : decoded
      } catch (e) {
        // Not fatal: the page still hydrates with empty data; navigation works.
        console.warn?.('oxide: window.__oxide.data decode failed; hydrating with {}', e)
      }
    }
    const input = {
      route: payload.route ?? '/',
      url: (w.location?.pathname ?? '/') + (w.location?.search ?? ''),
      params: {} as Record<string, string>,
      data,
    }
    try {
      adapter.hydrate(root, input as never)
      hydratedRoot = true
    } catch (e) {
      console.error?.('oxide: root hydration failed; SPA router not attached', e)
    }
  }

  // (b) Islands + deferred scripts — the same work `boot()` did, so
  //     `bootWithRouter` is a strict superset. A page with no `#root` /
  //     `window.__oxide` (pure SPA, or non-SSR HTML) still gets island
  //     hydration here.
  const loadChunk =
    w.__oxideBoot?.loadChunk ?? (async (c: string) => import(/* @vite-ignore */ c))
  try {
    hydrateIslands({ doc: doc as never, adapter: adapter as never, loadChunk })
    loadScripts({ doc: doc as never })
  } catch (e) {
    console.error?.('oxide: island hydration failed', e)
  }

  // (c) Router LAST — only when we actually hydrated a root. Otherwise leave
  //     the page on plain anchor navigation.
  if (!hydratedRoot) return () => {}

  // §8.4 loading boundaries: on nav start, find the longest matching loading
  // prefix and paint that component into an overlay div; on nav end, tear
  // the overlay down (right before the tree swap).
  let loadingDispose: (() => void) | undefined
  let loadingOverlay: HTMLElement | undefined

  function findLoading(url: string): unknown {
    const map = opts.loading
    if (!map) return undefined
    // Strip query/hash, then find the longest prefix key that is a
    // path-segment boundary of the URL. `'/'` matches everything.
    const path = url.split(/[?#]/, 1)[0] || '/'
    let bestKey: string | undefined
    for (const key of Object.keys(map)) {
      const k = key === '' ? '/' : key
      const isPrefix =
        k === '/' ? true : path === k || path.startsWith(k + '/')
      if (!isPrefix) continue
      if (!bestKey || k.length > (bestKey === '' ? 1 : bestKey.length)) {
        bestKey = key
      }
    }
    return bestKey !== undefined ? map[bestKey] : undefined
  }

  const onNavigateStart = (url: string): void => {
    if (typeof adapter.renderLoading !== 'function') return
    const Component = findLoading(url)
    if (Component === undefined) return
    // A dedicated overlay layer so the loading component does not collide
    // with the SSR root's own markup; removed on nav end.
    const overlay = doc.createElement('div')
    overlay.setAttribute('data-ox-loading', '')
    ;(doc.body ?? doc.documentElement)?.appendChild(overlay)
    loadingOverlay = overlay
    try {
      loadingDispose = adapter.renderLoading(overlay, { url, route: url, params: {}, data: {} }, Component)
    } catch (e) {
      console.warn?.('oxide: renderLoading threw', e)
      overlay.remove()
      loadingOverlay = undefined
    }
  }
  const onNavigateEnd = (): void => {
    try {
      loadingDispose?.()
    } catch {
      // A disposer that throws must not block navigation.
    }
    loadingDispose = undefined
    loadingOverlay?.remove()
    loadingOverlay = undefined
  }

  const router = createRouter({
    adapter: adapter as unknown as { update(i: unknown): void },
    onNavigateStart,
    onNavigateEnd,
  } as never)
  return router.attach()
}

/**
 * Install the boot runtime on `window` without running the island scan.
 * Called by the generated entry before `boot()` so `__oxideRegisterAdapter`
 * is available when the entry imports the framework adapter. No-op outside
 * the browser — safe to call from any module.
 */
export function installBootRuntime(): void {
  const w = maybeWindow()
  if (!w) return
  ensureInstalled(w)
}

// Install eagerly when the browser context is present, so a hand-written
// entry that imports `boot` and calls `window.__oxideRegisterAdapter`
// directly still works. The SSR/config-loader environment does not reach
// this branch.
{
  const w = maybeWindow()
  if (w) ensureInstalled(w)
}

export {}
