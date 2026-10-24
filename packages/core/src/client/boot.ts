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
import { fromBase64 } from '../bytes.ts'
import { loadScripts } from './scripts.ts'
import { setRouteComponentSource, isParamSegment, type ClientManifestLike } from './route-manifest.ts'
import { attachDeferGlue, type DeferGlue } from './defer-glue.ts'
import type { FrameworkAdapter } from '../types.ts'
import { loadMessages, setMessageLocales } from './messages.ts'

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
      doc,
      adapter: state.adapter,
      loadChunk: state.loadChunk,
    }
    hydrateIslands(opts)
    // §16.6: schedule server-rendered `<Script>` placeholders by strategy.
    loadScripts({ doc })
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
  // §16 / §16.4: warm the active locale's catalog. `<html lang>` is now
  // emitted from the resolved locale, so this loads the matching catalog
  // instead of always falling back to `en`. Fire-and-forget: `t()` returns
  // the key until the catalog lands, which is the documented degradation.
  void loadMessages()
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
 * - no `document` → no-op;
 * - no `window.__oxide` → islands + scripts only, no router (plain anchors);
 * - `window.__oxide` with `#root` → full mode: hydrate the root, islands,
 *   then the router. A hydration FAILURE leaves the SSR markup untouched and
 *   does NOT attach the router (the adapters' `update` would no-op and the
 *   URL would change without a DOM swap — worse than plain anchors);
 * - `window.__oxide` WITHOUT `#root` → router-only mode: nothing to
 *   hydrate, but the page still gets the loading overlays (§8.4), the defer
 *   glue (§7.1/§9.2), manifest-driven chunk preloading (§6.4) and the
 *   `ox:data` HMR path (§13.3). The adapters' `update` calls no-op without
 *   a mounted root.
 * - `data` in `window.__oxide` is base64 ODF, decoded via `decodeOdf`; a
 *   decode failure falls back to the raw string rather than throwing.
 *
 * Returns a disposer (the router's `dispose` + boot glue teardown), or a
 * no-op.
 */
export function bootWithRouter(
  adapter: FrameworkAdapter,
  opts: {
    doc?: Document
    /**
     * §8.4 loading boundaries: route-pattern → loading component map. Keys
     * are absolute paths whose segments may be `[name]` / `:name` tokens for
     * dynamic segments (`/blog/[slug]`), emitted by the boot entry from each
     * `loading.tsx`'s route directory. On a client navigation the longest
     * matching key (most matching segments; a key may cover the routes below
     * it) wins and `adapter.renderLoading` paints it into an overlay until
     * the tree swaps. Absent (or a framework without `renderLoading`) → no
     * loading boundary; the router still fires the hooks, they are just
     * no-ops here.
     */
    loading?: Record<string, unknown>
    /**
     * §6.4 manifest-driven chunk preloading + §8.4/§10.3 real component
     * resolution: the wire shape of `/client/client.manifest.json`
     * (`{ buildId, routes: { [routeId]: { entry_chunk, preload, css } } }`).
     * The router warms a route's chunks in parallel with the `/__data`
     * fetch, and the manifest is registered as the adapters'
     * {@link setRouteComponentSource | component source} so `update()`
     * renders the route's REAL page component instead of the IR registry.
     * Absent → data-only prefetch and the IR fallback.
     */
    manifest?: ClientManifestLike & {
      /**
       * §16.4: the locales this build emitted catalogs for. Absent when the
       * project has no i18n — `setMessageLocales(undefined)` then leaves the
       * fetch enabled (the pre-existing behaviour); an empty array would too,
       * so the CLI omits the field rather than sending `[]`.
       */
      i18n_locales?: string[]
    }
  } = {},
): () => void {
  const doc = opts.doc ?? (globalThis as { document?: Document }).document
  if (!doc) return () => {}
  // §16.4: the manifest says whether this build emitted any message
  // catalogs. Registering it before the first `loadMessages` is what stops a
  // non-i18n app from fetching a catalog that cannot exist (a guaranteed 404
  // and a console error).
  // §16.4: a *present* manifest without `i18n_locales` means the build
  // emitted no catalogs — skip the fetch. No manifest at all (dev, a test)
  // leaves the decision to the caller, which is the pre-existing behaviour.
  setMessageLocales(
    opts.manifest === undefined ? undefined : (opts.manifest.i18n_locales ?? []),
  )
  void loadMessages()
  const w = globalThis as {
    __oxide?: { route?: string; data?: string; layouts?: string[] }
    __oxideBoot?: BootState
    location?: { pathname: string; search: string }
  }

  const loadChunk =
    w.__oxideBoot?.loadChunk ?? (async (c: string) => import(/* @vite-ignore */ c))

  // §6.4 + §8.4/§10.3: register the manifest + chunk loader BEFORE the root
  // hydrates so every adapter's hydrate()/mount()/update() resolves REAL
  // page components through `componentFor` (route-manifest.ts) instead of
  // the IR Atlas registry. No manifest (dev server, static export without
  // one) → the adapters stay on the IR fallback, unchanged.
  setRouteComponentSource(opts.manifest ? { manifest: opts.manifest, loadChunk } : undefined)

  // (a) Root hydration — the router-independent part of boot. On a click the
  // router calls `adapter.update(input)`, which on the client side resolves
  // to the module-level `active?.update(input)` in each adapter's
  // `hydrate.ts` — and `active` is only set by `hydrate()` or `mount()`.
  // Without hydration the real-render path still works from `update()`
  // itself (it mounts into `#root` through the manifest), but the IR-only
  // path needs the handle, so a failed hydration still skips router
  // attachment below (the URL must never change without a DOM swap).
  //
  // Root hydration does NOT rewrite `#root.innerHTML` when the SSR markup is
  // already present, so `<ox-island>` hosts survive and the island scan
  // below still finds them.
  let hydratedRoot = false
  const root = doc.getElementById?.('root')
  const payload = w.__oxide
  let payloadData: unknown
  if (root && payload) {
    let data: unknown = {}
    // §8.2: the ODF envelope carries `{ route, params, data, layouts }`. The
    // `layouts` field is required to build the same component tree the
    // server rendered (`<RootLayout><Page/></RootLayout>`) — without it
    // React reports a mismatch (#418) on the server markup.
    let decoded: Record<string, unknown> | undefined
    if (typeof payload.data === 'string' && payload.data.length > 0) {
      try {
        // P-24: one shared chunked/native decoder, not a per-byte loop.
        decoded = decodeOdf(fromBase64(payload.data)) as Record<string, unknown>
        data = decoded && typeof decoded === 'object' && 'data' in decoded ? decoded.data : decoded
      } catch (e) {
        // Not fatal: the page still hydrates with empty data; navigation works.
        console.warn?.('oxide: window.__oxide.data decode failed; hydrating with {}', e)
      }
    }
    payloadData = data
    const odfLayouts =
      decoded && Array.isArray((decoded as { layouts?: unknown }).layouts)
        ? ((decoded as { layouts?: unknown }).layouts as string[])
        : []
    const jsLayouts = Array.isArray((payload as { layouts?: unknown }).layouts)
      ? ((payload as { layouts?: unknown }).layouts as string[])
      : []
    const layouts = odfLayouts.length > 0 ? odfLayouts : jsLayouts
    const input = {
      route: payload.route ?? '/',
      url: (w.location?.pathname ?? '/') + (w.location?.search ?? ''),
      params: {} as Record<string, string>,
      data,
      layouts,
    }
    try {
      adapter.hydrate(root, input)
      hydratedRoot = true
    } catch (e) {
      console.error?.('oxide: root hydration failed; SPA router not attached', e)
    }
  }

  // (b) Islands + deferred scripts — the same work `boot()` did, so
  //     `bootWithRouter` is a strict superset. A page with no `#root` /
  //     `window.__oxide` (pure SPA, or non-SSR HTML) still gets island
  //     hydration here.
  try {
    hydrateIslands({ doc, adapter, loadChunk })
    loadScripts({ doc })
  } catch (e) {
    console.error?.('oxide: island hydration failed', e)
  }

  // (c) Router attach — gated on the PAYLOAD, not the root (§8.4): a page
  //     with `window.__oxide` but no `#root` still gets loading overlays,
  //     the defer glue, manifest preloading and nav data (router-only
  //     mode). A hydration FAILURE with a root present is the one state
  //     that still skips attachment: `adapter.update` would no-op and the
  //     URL would change without a DOM swap.
  const hasPayload = payload !== undefined && payload !== null
  if (!hasPayload) return () => {}
  const routerOnly = !hydratedRoot
  if (routerOnly && root) return () => {}

  // (c.1) §7.1 / §9.2 boot-path glue init (router-only mode): the adapter's
  //       hydrate/update hooks never ran, so nothing wired the payload's
  //       `{__deferred: id}` markers or resolved the injected
  //       `<script type="application/oxide-deferred">`. Attach the core
  //       glue here so deferred boundaries resolve without islands or a
  //       page root.
  let bootGlue: DeferGlue | undefined
  if (routerOnly) {
    try {
      bootGlue = attachDeferGlue({ doc })
      if (payloadData !== undefined) bootGlue.wire(payloadData)
    } catch (e) {
      console.warn?.('oxide: boot defer glue failed', e)
      bootGlue = undefined
    }
  }

  // §8.4 loading boundaries: on nav start, find the longest matching loading
  // key and paint that component into an overlay div; on nav end, tear
  // the overlay down (right before the tree swap).
  //
  // KEY MATCHER (§8.4, the contract the CLI's boot entry documents): a key
  // is an absolute path whose segments may be `[name]` / `:name` tokens. A
  // literal key segment must equal the URL segment; a param key segment
  // matches exactly ONE non-empty URL segment; a key that is a proper
  // PREFIX of the URL matches too (a layout-level `loading.tsx` covers the
  // routes below it). The longest key (most matching segments) wins, ties
  // resolved to the key with more literal segments. Trailing slashes are
  // normalized on both sides (same behavior as the router's own loader);
  // the root key `/` matches every URL with the lowest priority.
  function findLoading(url: string): unknown {
    const map = opts.loading
    if (!map) return undefined
    // Strip query/hash and split into non-empty segments (`/blog/x` →
    // `['blog', 'x']`, `/` → `[]`).
    const urlSegs = (url.split(/[?#]/, 1)[0] || '/').split('/').filter((s) => s.length > 0)
    let bestKey: string | undefined
    let bestSegs = -1
    let bestStatic = -1
    for (const key of Object.keys(map)) {
      const keySegs = ((key || '/').replace(/\/+$/, '') || '/')
        .split('/')
        .filter((s) => s.length > 0)
      // A key longer than the URL can never match (prefix semantics).
      if (keySegs.length > urlSegs.length) continue
      let ok = true
      let staticCount = 0
      for (let i = 0; i < keySegs.length; i++) {
        const k = keySegs[i]
        const u = urlSegs[i]
        if (k === undefined || u === undefined) { ok = false; break }
        if (isParamSegment(k)) {
          if (u.length === 0) { ok = false; break }
          continue
        }
        if (k !== urlSegs[i]) { ok = false; break }
        staticCount++
      }
      if (!ok) continue
      if (
        keySegs.length > bestSegs ||
        (keySegs.length === bestSegs && staticCount > bestStatic)
      ) {
        bestKey = key
        bestSegs = keySegs.length
        bestStatic = staticCount
      }
    }
    return bestKey !== undefined ? map[bestKey] : undefined
  }

  let loadingDispose: (() => void) | undefined
  let loadingOverlay: HTMLElement | undefined

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

  // §13.3: the inline dev HMR client dispatches `ox:data` on the document
  // with `detail: { route, odf }`, where `odf` is base64 ODF (the same format
  // `/__data` returns). A caller may also dispatch `detail: { input }` with a
  // complete `ClientInput`. Both shapes feed the same `adapter.update` path a
  // navigation uses, so a loader edit re-renders the page in place.
  const onData = async (e: Event): Promise<void> => {
    const detail = (e as CustomEvent<{ input?: unknown; route?: string; odf?: string }>).detail
    if (!detail || typeof detail !== 'object') return

    let input:
      | { route: string; url: string; params: Record<string, string | string[]>; data: unknown }
      | undefined

    if ('input' in detail && detail.input) {
      // Whole-input shape: caller already built a ClientInput.
      const i = detail.input as {
        route?: unknown
        url?: unknown
        params?: unknown
        data?: unknown
      }
      if (typeof i.route !== 'string') return
      input = {
        route: i.route,
        url: typeof i.url === 'string' ? i.url : (globalThis as { location?: { href: string } }).location?.href ?? '/',
        params: (i.params as Record<string, string | string[]>) ?? {},
        data: i.data,
      }
    } else if (typeof detail.odf === 'string' && detail.odf.length > 0) {
      // Inline HMR shape: `{ route, odf }` with base64 ODF.
      let data: unknown
      try {
        // P-24: shared decoder (see the hydration path above).
        const decoded = decodeOdf(fromBase64(detail.odf)) as Record<string, unknown>
        // The ODF envelope is `{ route, url, params, data }`; unwrap `data`.
        data = decoded && typeof decoded === 'object' && 'data' in decoded ? decoded.data : decoded
      } catch (err) {
        console.warn?.('oxide: ox:data odf decode failed', err)
        return
      }
      const route = detail.route
      if (typeof route !== 'string') return
      const loc = (globalThis as { location?: { href: string } }).location
      const winParams = (globalThis as { __oxide?: { params?: unknown } }).__oxide?.params
      input = {
        route,
        url: loc?.href ?? '/',
        params: (winParams as Record<string, string | string[]> | undefined) ?? {},
        data,
      }
    } else {
      return
    }

    try {
      adapter.update(input)
    } catch (err) {
      console.warn?.('oxide: ox:data update threw', err)
    }
  }
  doc.addEventListener('ox:data', onData as EventListener)

  const router = createRouter({
    adapter,
    onNavigateStart,
    onNavigateEnd,
    ...(opts.manifest ? { manifest: opts.manifest } : {}),
  })
  const disposeRouter = router.attach()
  return () => {
    disposeRouter()
    doc.removeEventListener('ox:data', onData as EventListener)
    bootGlue?.dispose()
    bootGlue = undefined
  }
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
