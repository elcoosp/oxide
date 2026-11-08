// The client side of the build id contract (spec v0.2, §V2-A04 step 4).
//
// The hydration bootstrap advertises the canonical build id as the fourth
// `window.__oxide` field (`render::hydration_bootstrap_with_build`); the
// helpers here read it and put it on every data-plane fetch (`/__data`,
// `/_oxide/fn/*`), and turn a stale-generation 409 into one guarded reload.
//
// Module-safety contract (same as `boot.ts`): import-safe in the SSR /
// config-loader Node environment — everything touches runtime-guarded
// globals, no top-level side effects.

/** The `window.__oxide` bootstrap shape the render crate emits (§9.5, §V2-A04). */
type OxideBootstrap = {
  route?: string
  build?: string
  data?: unknown
}

function readBootstrap(): OxideBootstrap | undefined {
  const g = globalThis as { __oxide?: OxideBootstrap }
  return g.__oxide
}

/** The canonical build id this page was rendered by, when advertised. */
export function buildId(): string | undefined {
  return readBootstrap()?.build
}

/**
 * Add `x-oxide-build` to a fetch init; use for every data-plane request.
 * Without an advertised build id (curl, tests, old clients, prerendered
 * pages before the A01-phase-3 wiring) the init passes through untouched —
 * the server allows header-less requests by design.
 */
export function withBuild(init: RequestInit = {}): RequestInit {
  const id = buildId()
  if (!id) return init
  const h = new Headers(init.headers)
  h.set('x-oxide-build', id)
  return { ...init, headers: h }
}

/**
 * On 409 + `x-oxide-build-stale: true`: reload once per build id, never
 * loop. Returns true when a reload was started — callers must abort the
 * interrupted navigation/action/call instead of parsing the response.
 *
 * The reload default degrades to a no-op where there is no `location`
 * (Node tests); the one-reload-per-build guard uses `sessionStorage` and
 * degrades to "reload once" when storage is unavailable or blocked.
 */
export function handleStale(res: Response, reload: () => void = () => globalThis.location?.reload()): boolean {
  if (res.status !== 409 || res.headers.get('x-oxide-build-stale') !== 'true') return false
  const key = 'ox:reloaded:' + (buildId() ?? 'none')
  try {
    const store = globalThis.sessionStorage as Storage | undefined
    if (!store) throw new Error('no session storage')
    if (store.getItem(key)) return false
    store.setItem(key, '1')
  } catch {
    // Storage missing or blocked: fall through and reload once — the
    // server's 409 is authoritative, a loop needs the guard we could not
    // write, and without a reload the client stays stale forever.
  }
  reload()
  return true
}
