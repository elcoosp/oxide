// The client side of the live contract (spec v0.2, §V2-F05 step 7).
//
// A route marked live re-runs its loaders when a tag it depends on is
// revalidated; this module pushes the new data into open tabs with no
// hand-written socket code: an `EventSource` over `GET /__oxide/live`
// carrying the route id, the current URL and the build id in the query
// (EventSource cannot set headers, so the A04 build check reads the query
// — see `oxide_fw_dispatch::build::stage_build_check`).
//
// Wire contract (server side: `oxide_fw_server::live`):
//   event: data\nid: N\ndata: <base64 ODF>\n\n   fresh snapshot (full snapshot on connect — no replay)
//   event: ping\ndata: 1\n\n                     keepalive every live.keepaliveSeconds
//   event: stale\ndata: <new build id>\n\n       build changed; reload once (A04)
//
// Module-safety contract (same as `boot.ts` / `build.ts`): import-safe in
// the SSR / config-loader Node environment — every browser global is
// touched through guarded accessors, no top-level side effects. The
// `EventSource` constructor and the `document` visibility hook are
// injectable so the Node test runner can pin the behavior without a DOM.

import { decodeOdf } from './odf-decode.ts'
import { buildId } from './build.ts'

/** Decode one SSE `data:` payload into a JS value (see `decodeOdf`). */
export type LiveDecode = (payload: string, last: unknown) => unknown

type MinimalEventSource = {
  close(): void
  addEventListener(type: string, cb: (ev: { data?: string }) => void): void
  onerror: ((ev?: unknown) => void) | null
}

type EventSourceLike = {
  new (url: string): MinimalEventSource
}

/** A document subset for the visibility guard (injectable for tests). */
type VisibilityDocument = {
  visibilityState: string
  addEventListener(type: string, cb: () => void): void
  removeEventListener(type: string, cb: () => void): void
}

export type ConnectLiveOptions = {
  /**
   * Decode an SSE `data:` payload against the previously applied value.
   * Defaults to the base64 ODF snapshot decode; the F06 patch transport
   * swaps this single function.
   */
  decode?: LiveDecode
  /** The EventSource constructor (default: `globalThis.EventSource`). */
  EventSource?: EventSourceLike
  /** The visibility document (default: `globalThis.document`). */
  document?: VisibilityDocument
  /** Reload hook (default: `location.reload()`; Node-test-safe). */
  reload?: () => void
}

/** Node-test-safe base64 → bytes (the islands.ts pattern). */
function b64Bytes(b64: string): Uint8Array {
  const g = globalThis as {
    atob?: (s: string) => string
    Buffer?: { from(s: string, enc: string): Uint8Array }
  }
  if (typeof g.atob === 'function') {
    const bin = g.atob(b64)
    const out = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  }
  if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, 'base64'))
  throw new TypeError('live: no base64 decoder available (atob or Buffer)')
}

/** The default decode: every snapshot is a full base64 ODF payload. */
const decodeOdfSnapshot: LiveDecode = (payload) => decodeOdf(b64Bytes(payload))

function currentUrl(): string {
  const g = globalThis as { location?: { pathname: string; search: string } }
  if (!g.location) return ''
  return g.location.pathname + g.location.search
}

/**
 * Live updates for one route. Returns a disposer: call it on navigation
 * away from the route (the router owns the lifecycle) — it closes the
 * stream, cancels the pending reconnect and the visibility guard, and
 * removes its listeners.
 */
export function connectLive(
  route: string,
  apply: (data: unknown) => void,
  opts: ConnectLiveOptions = {},
): () => void {
  const decode = opts.decode ?? decodeOdfSnapshot
  const Ctor = opts.EventSource ??
    (globalThis as { EventSource?: EventSourceLike }).EventSource
  const doc = opts.document ??
    (globalThis as { document?: VisibilityDocument }).document
  const reload = opts.reload ?? (() => (globalThis as { location?: { reload(): void } }).location?.reload())

  let last: unknown
  let es: MinimalEventSource | undefined
  let reconnect: ReturnType<typeof setTimeout> | undefined
  let hideTimer: ReturnType<typeof setTimeout> | undefined
  let hiddenClosed = false
  let closed = false

  const open = (): void => {
    if (closed) return
    if (!Ctor) return // no transport in this environment: stay inert
    const url =
      '/__oxide/live?route=' +
      encodeURIComponent(route) +
      '&url=' +
      encodeURIComponent(currentUrl()) +
      '&build=' +
      encodeURIComponent(buildId() ?? '')
    es = new Ctor(url)
    es.addEventListener('data', (e) => {
      last = decode(e.data ?? '', last)
      apply(last)
    })
    es.addEventListener('stale', () => {
      // Build changed (A04): reload once — the fresh page reconnects with
      // the new build id. The stream is dead either way.
      es?.close()
      reload()
    })
    // Jittered reconnect; the server answers a fresh connection with a
    // full data snapshot, so a drop never needs a client-side replay.
    es.onerror = () => {
      es?.close()
      if (closed) return
      reconnect = setTimeout(open, 1000 + Math.random() * 2000)
    }
  }

  // Spec step 7: respect document.visibilityState — close when hidden for
  // more than 30 s, reconnect on visibilitychange.
  const onVisibility = (): void => {
    if (!doc) return
    if (doc.visibilityState === 'hidden') {
      if (hideTimer === undefined) {
        hideTimer = setTimeout(() => {
          hideTimer = undefined
          hiddenClosed = true
          es?.close()
        }, 30_000)
      }
    } else {
      if (hideTimer !== undefined) {
        clearTimeout(hideTimer)
        hideTimer = undefined
      }
      // The hidden-close killed the stream on purpose: reopen while the
      // tab is visible (the server answers with a full snapshot).
      if (hiddenClosed && !closed) {
        hiddenClosed = false
        open()
      }
    }
  }

  doc?.addEventListener('visibilitychange', onVisibility)
  open()

  return () => {
    closed = true
    es?.close()
    if (reconnect !== undefined) clearTimeout(reconnect)
    if (hideTimer !== undefined) clearTimeout(hideTimer)
    doc?.removeEventListener('visibilitychange', onVisibility)
  }
}
