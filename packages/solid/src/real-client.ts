// @oxide/solid real client page root (spec §8.4 + §10.3).
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

async function load(): Promise<{
  web: SolidWebClientLike
  create: (type: unknown, props: unknown) => unknown
} | undefined> {
  if (cachedWeb && cachedCreate) return { web: cachedWeb, create: cachedCreate }
  if (attempted) return undefined
  attempted = true
  // Same package order as ./hydrate-island-real.ts: Solid 2's dedicated web
  // runtime first, Solid 1's `solid-js/web` as the fallback.
  let web: SolidWebClientLike | undefined
  try {
    web = (await import('@solidjs/web')) as unknown as SolidWebClientLike
  } catch {
    try {
      web = (await import('@solidjs/web')) as unknown as SolidWebClientLike
    } catch {
      return undefined
    }
  }
  if (typeof web?.hydrate !== 'function' || typeof web.render !== 'function') return undefined
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
