// @oxide/solid real client page root (spec §8.4 + §10.3).

/** Runtime type guard for @solidjs/web's client render surface. */
function isSolidWebClientLike(v: unknown): v is SolidWebClientLike {
  if (v === null || typeof v !== 'object') return false
  return typeof Reflect.get(v, 'render') === 'function'
    || typeof Reflect.get(v, 'hydrate') === 'function'
}

//
// The manifest-driven `update()` / `hydrate()` / `mount()` path in
// ./hydrate.ts needs Solid's own client runtime to render (or hydrate) the
// REAL page component a route's client chunk exports — the same job
// `web.hydrate`/`web.render` does for islands in ./hydrate-island-real.ts,
// but for the whole page root. This module loads `@solidjs/web` (Solid 2)
// or `solid-js/web` (Solid 1) plus the `solid-js` core lazily and cached
// (the convention of ./real-ssr.ts on the server side): when the optional
// peer deps are absent the page path stays on the IR fallback and nothing
// here throws.

export interface SolidWebClientLike {
  hydrate(fn: () => unknown, element: Element): unknown
  render(fn: () => unknown, element: Element): unknown
}

export interface SolidCoreLike {
  createElement?: (type: unknown, props: unknown) => unknown
  createComponent?: (type: unknown, props: Record<string, unknown>) => unknown
}

/** Uniform handle the page path drives regardless of framework. */
export interface RealPageRoot {
  /** Re-render the current component with fresh props (navigation swap). */
  update(props: Record<string, unknown>): void
  /** Dispose the reactive root (route teardown / client-root dispose). */
  dispose(): void
}

let cachedWeb: SolidWebClientLike | undefined
let cachedCreate: ((type: unknown, props: unknown) => unknown) | undefined
let attempted = false

/** Structural type guard: `v` has at least one of the Solid core
 * element/component factory functions. Both the ESM namespace shape and
 * the interop `.default` shape expose these on the same keys. */
function isSolidCoreLike(v: unknown): v is SolidCoreLike {
  if (typeof v !== 'object' || v === null) return false
  const rec = v as Record<string, unknown>
  const hasElement = typeof rec.createElement === 'function'
  const hasComponent = typeof rec.createComponent === 'function'
  return hasElement || hasComponent
}

/** Import the Solid 2 web runtime. The specifier MUST stay a string literal:
 *  the dev-server prebundle rewrites static specifiers to `/@prebundle/*.js`
 *  URLs and Rolldown's prod bundler resolves them — a dynamic variable
 *  specifier (`import(name)`) survives compilation verbatim, the browser
 *  rejects it with `Failed to resolve module specifier` BEFORE any request,
 *  and `tryLoadWeb`'s catch turns that into `undefined` → the page path
 *  silently falls back to the IR tree (the "SSR said Home, the browser
 *  painted Atlas" regression the hmr-solid spec pins). TypeScript needs the
 *  `unknown` intermediate because Solid 2's types don't declare every export
 *  shape the runtime probes. */
async function tryLoadWebSolid2(): Promise<SolidWebClientLike | undefined> {
  try {
    const mod: unknown = await import('@solidjs/web')
    if (isSolidWebClientLike(mod) && typeof mod.hydrate === 'function' && typeof mod.render === 'function') {
      return mod
    }
    return undefined
  } catch {
    return undefined
  }
}

/** Solid 1's web runtime, as the fallback when `@solidjs/web` is absent.
 *  The specifier is built by CONCATENATION, deliberately: it must never
 *  appear as a static literal. As a literal, Rolldown's prod bundler tries
 *  to resolve `solid-js/web` — which does not exist under Solid 2 (the only
 *  runtime this adapter targets) — and the whole client build fails with
 *  `Could not resolve 'solid-js/web'`. A computed specifier is opaque to
 *  static analysis: the bundler leaves the import to runtime, where a
 *  browser cannot resolve a bare specifier and the catch below degrades to
 *  `undefined` (the primary `@solidjs/web` load already succeeded). The
 *  Node side resolves it from real node_modules via ./real-ssr.ts's
 *  variable-form loader. */
async function tryLoadWebSolid1(): Promise<SolidWebClientLike | undefined> {
  try {
    // 'solid-js/web' assembled at runtime (join, NOT concatenation: esbuild
    // constant-folds 'a' + 'b' back into the literal the doc-comment bans).
    const solid1Web = ['solid-js', 'web'].join('/')
    const mod: unknown = await import(solid1Web)
    if (isSolidWebClientLike(mod) && typeof mod.hydrate === 'function' && typeof mod.render === 'function') {
      return mod
    }
    return undefined
  } catch {
    return undefined
  }
}

async function load(): Promise<{
  web: SolidWebClientLike
  create: (type: unknown, props: unknown) => unknown
} | undefined> {
  if (cachedWeb && cachedCreate) return { web: cachedWeb, create: cachedCreate }
  if (attempted) return undefined
  attempted = true
  try {
    return await loadUncached()
  } finally {
    // §3.10 parity with hydrate-island-real.ts: a single transient failure
    // (dev-server restart, HMR outage) must not pin the page path to the IR
    // fallback for the rest of the session. A genuinely absent dep still
    // fails fast — the loader caches the failed specifier.
    if (!(cachedWeb && cachedCreate)) attempted = false
  }
}

async function loadUncached(): Promise<{
  web: SolidWebClientLike
  create: (type: unknown, props: unknown) => unknown
} | undefined> {
  // Same package order as ./hydrate-island-real.ts: Solid 2's dedicated web
  // runtime first, Solid 1's `solid-js/web` as the fallback. Both calls use
  // string-literal imports (see the loader doc-comments): a variable
  // specifier survives compilation verbatim and the browser cannot resolve
  // a bare specifier, which silently pinned every page to the IR fallback.
  const web = (await tryLoadWebSolid2()) ?? (await tryLoadWebSolid1())
  if (!web) return undefined
  // `createElement`/`createComponent` come from the solid-js core runtime
  // (the island path uses `createElement`; Solid keeps `createComponent` as
  // the canonical name).
  let core: SolidCoreLike | undefined
  try {
    const mod: unknown = await import('solid-js')
    // Two possible module shapes at runtime: an ESM namespace with named
    // exports (createElement/createComponent on `mod`), or an interop
    // wrapper with the real object under `.default`. Narrow structurally —
    // no `as unknown as` cast. `isSolidCoreLike` accepts either shape by
    // checking for the function fields the module actually exposes.
    if (typeof mod !== 'object' || mod === null) return undefined
    const modRec = mod as Record<string, unknown>
    const def = modRec.default
    if (isSolidCoreLike(def)) {
      core = def
    } else if (isSolidCoreLike(modRec)) {
      core = modRec
    } else {
      return undefined
    }
  } catch {
    return undefined
  }
  const create =
    typeof core?.createElement === 'function'
      ? core.createElement
      : typeof core?.createComponent === 'function'
        ? core.createComponent
        : undefined
  if (!create) return undefined
  cachedWeb = web
  cachedCreate = create as (type: unknown, props: unknown) => unknown
  return { web, create: cachedCreate }
}

/** True when a Solid web runtime + core are resolvable at runtime. */
export async function isSolidClientAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

interface SolidHydrationState {
  events: unknown[]
  completed: WeakSet<object>
  r: unknown
  fe(): void
  s?: unknown
  done?: boolean
  sp?: unknown[]
  sw?: (key?: string) => void
  modules?: Record<string, unknown>
  loading?: Record<string, Promise<unknown>>
}

/**
 * Install the `globalThis._$HY` hydration state `web.hydrate()` unconditionally
 * reads (`hy.s`, `hy.done`, later `hy.completed` via sharedConfig). Solid
 * creates it through the hydration script the SERVER embeds (`<Hydration/>
 * in solid-start`; `generateHydrationScript()`), and the Oxide shell does not
 * embed that script — without the state `hydrate()` throws
 * `Cannot read properties of undefined (reading 's')` and the page falls back
 * to the IR renderer. This is the same state shape the script installs
 * (including the pre-hydration click/input capture), done client-side.
 */
export function ensureSolidHydrationState(): void {
  const g = globalThis as { _$HY?: SolidHydrationState }
  if (g._$HY) return
  // `r` holds one promise per hydration request id; the Proxy swallows
  // rejections so a dangling `createResource` read can't crash the page.
  const r: Record<string | symbol, unknown> = {}
  const state: SolidHydrationState = {
    events: [],
    completed: new WeakSet(),
    r,
    fe() {},
    modules: {},
    loading: {},
  }
  state.r = new Proxy(r, {
    set(target, key, value) {
      if (value instanceof Promise) value.catch(() => {})
      target[key] = value
      return true
    },
  })
  // Pre-hydration event capture — identical to the script's arrow: clicks and
  // inputs that happen before hydration completes are queued so Solid can
  // replay them against the hydrated graph.
  const findClosest = (el: EventTarget | null): HTMLElement | null | undefined => {
    let cur = el as HTMLElement | null
    while (cur && typeof cur.hasAttribute === 'function') {
      if (cur.hasAttribute('_hk')) return cur
      const host = (cur as unknown as { host?: { nodeType?: number } }).host
      cur = (host && host.nodeType ? (host as HTMLElement) : cur.parentNode) as HTMLElement | null
    }
    return cur && typeof (cur as HTMLElement).hasAttribute === 'function' ? cur : null
  }
  if (typeof document !== 'undefined') {
    for (const type of ['click', 'input']) {
      document.addEventListener(type, (ev: Event) => {
        if (!state.events) return
        const path =
          typeof (ev as { composedPath?: () => EventTarget[] }).composedPath === 'function'
            ? (ev as { composedPath: () => EventTarget[] }).composedPath()
            : [ev.target]
        const closest = findClosest(path[0] ?? null)
        if (closest && !state.completed.has(closest)) state.events.push([closest, ev])
      })
    }
  }
  g._$HY = state
}

/**
 * Render (or hydrate) a REAL page component into the page container with
 * Solid's own client runtime. The component is wrapped in a zero-argument
 * factory — Solid's `render`/`hydrate` take `() => Node`.
 *
 * - `hydrate: true` → `web.hydrate` rebuilds the reactive graph against the
 *   server-rendered markup in `container` (§10.3 — the islands path).
 * - `hydrate: false` → `web.render` into the container.
 *
 * Returns `undefined` when Solid is not installed — callers fall back to
 * the IR renderer. Navigation updates dispose the previous root and render
 * fresh (signals restart per route, matching the "route swap" semantics of
 * the §8.4 navigation flow).
 */
export async function mountSolidPageRoot(
  container: Element,
  Component: unknown,
  props: Record<string, unknown>,
  opts: { hydrate?: boolean } = {},
): Promise<RealPageRoot | undefined> {
  const rt = await load()
  if (!rt) return undefined
  const el = container
  let disposer: unknown
  const renderFresh = (p: Record<string, unknown>): unknown => {
    el.innerHTML = ''
    return rt.web.render(() => rt.create(Component, p), el)
  }
  if (opts.hydrate) {
    ensureSolidHydrationState()
    disposer = rt.web.hydrate(() => rt.create(Component, props), el)
  } else {
    disposer = renderFresh(props)
  }
  const callDispose = (): void => {
    if (typeof disposer !== 'function') return
    try {
      ;(disposer as () => void)()
    } catch {
      // A dispose that throws (detached container) must not break teardown.
    }
  }
  return {
    update(next: Record<string, unknown>): void {
      callDispose()
      disposer = renderFresh(next)
    },
    dispose(): void {
      callDispose()
    },
  }
}
