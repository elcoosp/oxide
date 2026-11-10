// HMR client runtime (spec §13.3, message schema **v1**). Connects to the
// dev server's `/_oxide/hmr` WebSocket, applies each `HmrMessage`, and
// renders a diagnostic overlay on an `Error` message.
//
// Message kinds (mirror `oxide_dev::HmrMessage`, schema v1 — every frame
// carries a `{ v: 1 }` envelope):
//   - `update`            apply N update entries (`kind: 'js' | 'css' |
//                         'data'`): a js entry re-imports `acceptedPath`
//                         through the HMR runtime (adapter HMR applies it in
//                         place), a css entry swaps the styles without
//                         reload, a data entry re-fetches `/__data`
//   - `ox:data`           re-fetch `/__data` for the current route (or decode
//                         the pushed base64 ODF `data`) and dispatch `ox:data`
//                         with a fresh ClientInput — `bootWithRouter` feeds it
//                         to `adapter.update` so a loader edit re-renders in
//                         place
//   - `full-reload`       reload the page
//   - `error`             show the diagnostic overlay
//   - `connected`         the build id (informational)
//
// Read-compat: a stale tab (kept open across a CLI rebuild) still holds the
// previous session's runtime, so the legacy v0 spellings (`module-update`,
// `css-update`, `batch-module-update`, `route-data-update`,
// `server-module-swap`) normalize into the v1 shapes instead of being
// dropped silently — see `normalizeLegacyFrame` and docs/deviations.md.
//
// Fully guarded: a no-op outside the browser, so `@oxide/react` can re-export
// it and the config loader can evaluate it in the Node worker.

import { decodeOdf } from './odf-decode.ts'

export interface HmrMessage {
  type: string
  [k: string]: unknown
}

export interface HmrOptions {
  /** The WebSocket URL. Default: `ws(s)://<host>/_oxide/hmr`. */
  url?: string
  /** Reconnect delay in ms (default 1000). */
  reconnectMs?: number
  /** Called after a CSS swap (tests / devtools). */
  onCssUpdate?: (path: string) => void
  /** Called after a route-data update. */
  onRouteData?: (route: string, odfB64: string) => void
  /** Called for a module update (§13.3). v0.1 reloads on a module change,
   * so only the path is meaningful; the empty code/boundary fields are gone. */
  onModuleUpdate?: (path: string) => void
  /** Overridable reload (tests). */
  reload?: () => void
  /** The overlay root element (created if absent). */
  overlayParent?: HTMLElement
}

/** One §13.3 v1 update entry (`updates[]` inside an `update` frame). */
export interface HmrUpdateEntry {
  /** The client update mechanism: `'js' | 'css' | 'data'`. */
  kind?: string
  /** The changed file (or, for `data`, the route the loader belongs to). */
  path?: string
  /** What the client re-imports or swaps (`acceptedPath` on the wire). */
  acceptedPath?: string
  /** Publish time, unix milliseconds. */
  timestamp?: number
  /** Content hash of the changed file (cache busting). */
  hash?: string
}

/** Default WebSocket URL for the current page. */
function defaultUrl(): string {
  const g = globalThis as { location?: { protocol: string; host: string } }
  const loc = g.location
  if (!loc) return ''
  const scheme = loc.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${loc.host}/_oxide/hmr`
}

/**
 * §1.3: does a route PATTERN (`/blog/[slug]`, `/`, `/greeting`) match the
 * concrete `pathname` the browser is on? Segment-wise: a static segment must
 * equal; `[param]` / `:param` / `*` match one non-empty segment. Used to drop
 * `route-data-update` messages for routes the viewer is NOT on — the boot
 * window (and an edit to another page's loader) publishes fresh loader data
 * for foreign routes, and blindly feeding it to `adapter.update` rendered
 * THAT route in place: a phantom navigation with no URL change that replaced
 * the visible page with a different one.
 */
export function routePatternMatchesPath(pattern: string, pathname: string): boolean {
  const pSegs = pattern.split('/').filter((s) => s.length > 0)
  const pathSegs = pathname
    .split('?')[0]
    ?.split('#')[0]
    ?.split('/')
    .filter((s) => s.length > 0) ?? []
  if (pSegs.length !== pathSegs.length) return false
  for (let i = 0; i < pSegs.length; i++) {
    const p = pSegs[i] ?? ''
    const seg = pathSegs[i] ?? ''
    if (p.startsWith('[') || p.startsWith(':') || p === '*' || p.startsWith('*')) continue
    if (p !== seg) return false
  }
  return true
}

/** Swap the <link rel=stylesheet> whose href matches `path` (query-stripped). */
function swapCss(doc: Document, path: string): boolean {
  const bare = path.split('?').at(0) ?? ''
  const links = doc.querySelectorAll('link[rel="stylesheet"]')
  for (let i = 0; i < links.length; i++) {
    const el = links[i]
    if (el === undefined) continue
    const linkEl = el as HTMLLinkElement
    const rawHref = linkEl.getAttribute('href') ?? ''
    const href = rawHref.split('?').at(0) ?? ''
    if (href === bare || href.endsWith(bare)) {
      linkEl.setAttribute('href', `${bare}?t=${Date.now()}`)
      return true
    }
  }
  return false
}

/** Render (or replace) the diagnostic overlay. */
export function showOverlay(diag: { code?: string; message?: string; fix?: string }, parent?: HTMLElement): HTMLElement | undefined {
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return undefined
  const root = parent ?? doc.body
  const id = 'ox-hmr-overlay'
  let el = doc.getElementById(id)
  if (!el) {
    el = doc.createElement('div')
    el.id = id
    ;(el as HTMLElement).style.cssText =
      'position:fixed;inset:auto 16px 16px 16px;z-index:2147483647;' +
      'background:#1a0f0f;color:#ffd7d7;border:1px solid #a33;border-radius:8px;' +
      'font:13px/1.5 ui-monospace,monospace;padding:14px 16px;max-height:40vh;overflow:auto;' +
      'box-shadow:0 8px 24px rgba(0,0,0,.5)'
    root.appendChild(el)
  }
  const capturedEl = el
  const code = diag.code ?? 'ERROR'
  const msg = diag.message ?? ''
  el.innerHTML = ''
  const head = doc.createElement('div')
  head.style.cssText = 'display:flex;justify-content:space-between;align-items:center;margin-bottom:8px'
  const title = doc.createElement('strong')
  title.textContent = code
  const close = doc.createElement('button')
  close.textContent = '×'
  close.style.cssText = 'background:none;border:0;color:inherit;font-size:18px;cursor:pointer'
  close.addEventListener('click', () => capturedEl.remove())
  head.appendChild(title)
  head.appendChild(close)
  const body = doc.createElement('pre')
  body.style.cssText = 'white-space:pre-wrap;word-break:break-word;margin:0'
  body.textContent = msg
  el.appendChild(head)
  el.appendChild(body)
  if (diag.fix) {
    const fix = doc.createElement('div')
    fix.style.cssText = 'margin-top:8px;color:#9fdc9f'
    fix.textContent = `fix: ${diag.fix}`
    el.appendChild(fix)
  }
  return el
}

/**
 * §13.3 `route-data-update`: a page loader changed in dev. The dev server
 * publishes the route as a FILE path (not a URL) alongside the freshly
 * rendered loader payload as `odf_b64`, so the refresh prefers decoding that
 * payload inline and only falls back to re-fetching `/__data` for the CURRENT
 * location when it is absent, then dispatches
 * `ox:data` on the document with the normalized ClientInput — the same
 * event `bootWithRouter` consumes to call `adapter.update` (the same data
 * path a navigation uses, §8.4). All failures are silent: an HMR data
 * refresh is best-effort and must never break the socket loop.
 */
async function refreshRouteData(route: string, odfB64: string): Promise<void> {
  const g = globalThis as {
    document?: { dispatchEvent?(ev: unknown): boolean }
    location?: { pathname: string; search: string }
    fetch?: typeof fetch
    CustomEvent?: new (type: string, opts: { detail?: unknown }) => unknown
  }
  // §1.3: a loader edit for a route the viewer is NOT on must not reach
  // `adapter.update` — that renders the foreign route in place (phantom
  // navigation). Drop it here; the data is served fresh on the next real
  // navigation anyway.
  const locPath = g.location?.pathname
  if (locPath && !routePatternMatchesPath(route, locPath)) return
  const path = (locPath ?? '/') + (g.location?.search ?? '')
  let decoded: unknown
  if (odfB64.length > 0) {
    try {
      const bin = atob(odfB64)
      const bytes = new Uint8Array(bin.length)
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
      decoded = decodeOdf(bytes)
    } catch {
      return // a malformed pushed payload is ignored, not fatal
    }
  } else {
    if (typeof g.fetch !== 'function') return
    try {
      const res = await g.fetch(`/__data?route=${encodeURIComponent(path)}`, {
        headers: { accept: 'application/oxide+odf, application/json' },
      })
      if (!res.ok) return
      decoded = decodeOdf(new Uint8Array(await res.arrayBuffer()))
    } catch {
      return
    }
  }
  // Same normalization as the router's `normalizeInput` (§8.4): the /__data
  // envelope is `{ route, url, params, data }`; a bare payload IS the data.
  const d = (typeof decoded === 'object' && decoded !== null ? decoded : {}) as Record<string, unknown>
  const input = {
    route: (d.route as string) ?? path,
    url: (d.url as string) ?? path,
    params: (d.params as Record<string, string | string[]>) ?? {},
    data: 'data' in d ? d.data : decoded,
  }
  try {
    const detail = { route: path, input }
    const ev = typeof g.CustomEvent === 'function'
      ? new g.CustomEvent('ox:data', { detail })
      : { type: 'ox:data', detail }
    g.document?.dispatchEvent?.(ev)
  } catch {
    // A listener throwing must not break the HMR socket loop.
  }
}

/**
 * Remove the diagnostic overlay if present. Called when a subsequent
 * `update` frame (or successful re-import) proves the earlier diagnostic is
 * stale — the user fixed the file and the next edit applied cleanly.
 */
export function clearOverlay(parent?: HTMLElement): void {
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return
  const root = parent ?? doc.body
  const el = root?.querySelector?.('#ox-hmr-overlay') ?? doc.getElementById('ox-hmr-overlay')
  if (el) el.remove()
}

/**
 * Read-compat (§13.3 v1 flip): normalize the retired v0 spellings into the
 * v1 shapes so ONE code path applies both generations. A stale tab holds
 * the previous session's runtime but receives the CURRENT server's frames —
 * and vice versa during a CLI restart window — so the legacy frames must
 * keep working until the schema-window compat is retired (tracked in
 * docs/deviations.md).
 */
function normalizeLegacyFrame(msg: HmrMessage): HmrMessage {
  switch (msg.type) {
    case 'module-update':
      return {
        type: 'update',
        updates: [
          {
            kind: 'js',
            path: String(msg.path ?? ''),
            acceptedPath: typeof msg.url === 'string' ? msg.url : '',
          },
        ],
      }
    case 'css-update':
      return {
        type: 'update',
        updates: [
          { kind: 'css', path: String(msg.path ?? ''), acceptedPath: String(msg.path ?? '') },
        ],
      }
    case 'batch-module-update': {
      const rec = msg as unknown as { items?: unknown }
      const items = Array.isArray(rec.items) ? (rec.items as Array<Record<string, unknown>>) : []
      return {
        type: 'update',
        updates: items.map((it) => ({
          kind: 'js',
          path: String(it.path ?? ''),
          acceptedPath: typeof it.url === 'string' ? it.url : '',
        })),
      }
    }
    case 'route-data-update': {
      const route = String(msg.route ?? '')
      const odf = String(msg.odf_b64 ?? '')
      // A pushed payload rides as `ox:data`; a payload-less frame is a
      // data entry the client re-fetches.
      return odf
        ? { type: 'ox:data', route, data: odf }
        : { type: 'update', updates: [{ kind: 'data', path: route }] }
    }
    case 'server-module-swap':
      // Retired on the v1 wire: the server-side invalidation owns the swap
      // and the client arm was a no-op — an empty update is exactly that.
      return { type: 'update', updates: [] }
    default:
      return msg
  }
}

/** Apply one HMR message. Returns true when handled. */
export function applyHmrMessage(msg: HmrMessage, opts: HmrOptions = {}): boolean {
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return false
  const frame = normalizeLegacyFrame(msg)
  switch (frame.type) {
    case 'update': {
      const rec = frame as unknown as { updates?: unknown }
      const updates = Array.isArray(rec.updates) ? (rec.updates as HmrUpdateEntry[]) : []
      // A fresh update frame supersedes any earlier diagnostic; the user
      // fixed the file, the rebuild succeeded, the overlay is stale.
      clearOverlay(opts.overlayParent)
      const hmr = (globalThis as { __oxide_hmr__?: { reloadModule?: (u: string) => Promise<void> } }).__oxide_hmr__
      const reload = opts.reload ?? (() => (globalThis as { location?: { reload(): void } }).location?.reload())
      const fallbackUrls: string[] = []
      let usedRuntime = false
      for (const entry of updates) {
        const path = typeof entry.path === 'string' ? entry.path : ''
        const accepted = typeof entry.acceptedPath === 'string' ? entry.acceptedPath : ''
        const kind = typeof entry.kind === 'string' ? entry.kind : 'js'
        if (kind === 'css') {
          // §13.3: a stylesheet swaps without reload. The mechanism follows
          // acceptedPath: a module-owned swap re-imports the /@css/ module
          // (style injection); a bare path swaps the <link> href
          // (cache-busted).
          if (accepted && accepted !== path && typeof hmr?.reloadModule === 'function') {
            usedRuntime = true
            void hmr.reloadModule(accepted)
          } else {
            swapCss(doc, accepted || path)
          }
          opts.onCssUpdate?.(accepted || path)
        } else if (kind === 'data') {
          opts.onRouteData?.(path, '')
          // §13.3: re-fetch (or decode) the fresh loader data and re-dispatch
          // it through the same event the boot machinery consumes. Async and
          // silent — the message itself is handled synchronously. §1.3: the
          // route is passed through so a foreign route's data is dropped
          // instead of re-rendering the page (see refreshRouteData).
          void refreshRouteData(path, '')
        } else {
          if (accepted && typeof hmr?.reloadModule === 'function') {
            usedRuntime = true
            void hmr.reloadModule(accepted)
          } else if (accepted) {
            fallbackUrls.push(accepted)
          } else {
            opts.onModuleUpdate?.(path)
          }
        }
      }
      // Entries that could not ride the runtime fall back the way the
      // singular module-update did: the callback when one exists, one page
      // reload otherwise (never N reloads for one frame).
      if (fallbackUrls.length > 0 && !usedRuntime) {
        if (opts.onModuleUpdate) {
          for (const url of fallbackUrls) opts.onModuleUpdate(url)
        } else {
          reload?.()
        }
      }
      return true
    }
    case 'ox:data': {
      const route = String(frame.route ?? '')
      const odf = String(frame.data ?? '')
      opts.onRouteData?.(route, odf)
      void refreshRouteData(route, odf)
      return true
    }
    case 'full-reload': {
      const reload = opts.reload ?? (() => (globalThis as { location?: { reload(): void } }).location?.reload())
      reload?.()
      return true
    }
    case 'error': {
      showOverlay((frame.diagnostic as { code?: string; message?: string; fix?: string }) ?? {}, opts.overlayParent)
      return true
    }
    case 'connected':
      return true
    default:
      return false
  }
}

/** Open the HMR WebSocket; returns a disposer. No-op outside a browser. */
export function connectHmr(opts: HmrOptions = {}): () => void {
  const g = globalThis as { WebSocket?: new (url: string) => HmrSocket; document?: Document }
  if (typeof g.WebSocket !== 'function' || !g.document) return () => {}
  const url = opts.url ?? defaultUrl()
  if (!url) return () => {}
  let closed = false
  let socket: HmrSocket | undefined
  const open = () => {
    if (closed) return
    socket = new g.WebSocket!(url)
    socket.onmessage = (ev: { data: unknown }) => {
      try {
        const msg = JSON.parse(String(ev.data)) as HmrMessage
        applyHmrMessage(msg, opts)
      } catch {
        // A malformed frame is ignored, not fatal.
      }
    }
    socket.onclose = () => {
      socket = undefined
      if (!closed) setTimeout(open, opts.reconnectMs ?? 1000)
    }
    socket.onerror = () => {
      socket?.close()
    }
  }
  open()
  return () => {
    closed = true
    socket?.close()
  }
}

interface HmrSocket {
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: (() => void) | null
  onerror: (() => void) | null
  close(): void
}
