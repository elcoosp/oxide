// HMR client runtime (spec §13.3). Connects to the dev server's `/_oxide/hmr`
// WebSocket, applies each `HmrMessage`, and renders a diagnostic overlay on an
// `Error` message.
//
// Message kinds (mirror `oxide_dev::HmrMessage`):
//   - `css-update`        swap a <link> href to the new CSS
//   - `route-data-update` re-fetch `/__data` for the current route (or decode
//                         the pushed `odf_b64`) and dispatch `ox:data` with a
//                         fresh ClientInput — `bootWithRouter` feeds it to
//                         `adapter.update` so a loader edit re-renders in place
//   - `module-update`     apply a component module (via the adapter's HMR hook)
//   - `server-module-swap` a server module changed — full reload
//   - `full-reload`       reload the page
//   - `error`             show the diagnostic overlay
//   - `connected`         the build id (informational)
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

/** Default WebSocket URL for the current page. */
function defaultUrl(): string {
  const g = globalThis as { location?: { protocol: string; host: string } }
  const loc = g.location
  if (!loc) return ''
  const scheme = loc.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${loc.host}/_oxide/hmr`
}

/** Swap the <link rel=stylesheet> whose href matches `path` (query-stripped). */
function swapCss(doc: Document, path: string): boolean {
  const bare = path.split('?')[0]
  const links = doc.querySelectorAll('link[rel="stylesheet"]')
  for (let i = 0; i < links.length; i++) {
    const el = links[i] as HTMLLinkElement
    const href = (el.getAttribute('href') ?? '').split('?')[0]
    if (href === bare || href.endsWith(bare)) {
      el.setAttribute('href', `${bare}?t=${Date.now()}`)
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
  close.addEventListener('click', () => el!.remove())
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
 * publishes the route as a FILE path (not a URL) and an empty `odf_b64`
 * today, so the actionable refresh is: decode a non-empty `odf_b64` inline,
 * otherwise re-fetch `/__data` for the CURRENT location, then dispatch
 * `ox:data` on the document with the normalized ClientInput — the same
 * event `bootWithRouter` consumes to call `adapter.update` (the same data
 * path a navigation uses, §8.4). All failures are silent: an HMR data
 * refresh is best-effort and must never break the socket loop.
 */
async function refreshRouteData(odfB64: string): Promise<void> {
  const g = globalThis as {
    document?: { dispatchEvent?(ev: unknown): boolean }
    location?: { pathname: string; search: string }
    fetch?: typeof fetch
    CustomEvent?: new (type: string, opts: { detail?: unknown }) => unknown
  }
  const path = (g.location?.pathname ?? '/') + (g.location?.search ?? '')
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

/** Apply one HMR message. Returns true when handled. */
export function applyHmrMessage(msg: HmrMessage, opts: HmrOptions = {}): boolean {
  const doc = (globalThis as { document?: Document }).document
  if (!doc) return false
  switch (msg.type) {
    case 'css-update': {
      const path = String(msg.path ?? '')
      swapCss(doc, path)
      opts.onCssUpdate?.(path)
      return true
    }
    case 'route-data-update': {
      const route = String(msg.route ?? '')
      const odf = String(msg.odf_b64 ?? '')
      opts.onRouteData?.(route, odf)
      // §13.3: re-fetch (or decode) the fresh loader data and re-dispatch it
      // through the same event the boot machinery consumes. Async and
      // silent — the message itself is handled synchronously.
      void refreshRouteData(odf)
      return true
    }
    case 'module-update': {
      opts.onModuleUpdate?.(String(msg.path ?? ''))
      return true
    }
    case 'server-module-swap':
    case 'full-reload': {
      const reload = opts.reload ?? (() => (globalThis as { location?: { reload(): void } }).location?.reload())
      reload?.()
      return true
    }
    case 'error': {
      showOverlay((msg.diagnostic as { code?: string; message?: string; fix?: string }) ?? {}, opts.overlayParent)
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
