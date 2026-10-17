// HMR client runtime (spec §13.3). Connects to the dev server's `/_oxide/hmr`
// WebSocket, applies each `HmrMessage`, and renders a diagnostic overlay on an
// `Error` message.
//
// Message kinds (mirror `oxide_dev::HmrMessage`):
//   - `css-update`        swap a <link> href to the new CSS
//   - `route-data-update` re-render with the new ODF data (the router's job)
//   - `module-update`     apply a component module (via the adapter's HMR hook)
//   - `server-module-swap` a server module changed — full reload
//   - `full-reload`       reload the page
//   - `error`             show the diagnostic overlay
//   - `connected`         the build id (informational)
//
// Fully guarded: a no-op outside the browser, so `@oxide/react` can re-export
// it and the config loader can evaluate it in the Node worker.

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
  /** Called for a module update; the adapter's HMR hook. */
  onModuleUpdate?: (path: string, boundary: string, code: string) => void
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
      return true
    }
    case 'module-update': {
      opts.onModuleUpdate?.(
        String(msg.path ?? ''),
        String(msg.boundary ?? ''),
        String(msg.code ?? ''),
      )
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
