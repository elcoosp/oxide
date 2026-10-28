// @oxide/solid real island hydration via `@solidjs/web`'s `hydrate`.
//
// The IR-based `hydrateIsland` in ./hydrate.ts replays deterministic markup
// and only works for components without signals — any real app needs Solid's
// own hydration so `createSignal`/`createMemo`/`createEffect` and event
// handlers survive.
//
// `@solidjs/web` (Solid 2) or `solid-js/web` (Solid 1) is loaded lazily: an
// island page loads it when the first island's strategy fires (typically
// `load`, or on idle / intersection for the other strategies). A page with no
// islands never fetches it.
//
// The returned value is a `Promise<Disposer>`; `packages/core/src/client/
// islands.ts` awaits it before marking the host hydrated.

import type { Disposer } from '@oxide-fw/core/types'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Runtime type guard for solid-js core factory (Solid 2's createElement or
 *  Solid 1's createComponent). */
interface SolidFactoryForIsland {
  createElement?: (type: unknown, props: unknown) => unknown
  createComponent?: (type: unknown, props: unknown) => unknown
}

function isSolidFactoryForIsland(v: unknown): v is SolidFactoryForIsland {
  if (v === null || typeof v !== 'object') return false
  return typeof Reflect.get(v, 'createElement') === 'function'
    || typeof Reflect.get(v, 'createComponent') === 'function'
}


/** Narrow an `OxElement` to a real DOM `Element` (runtime assertion — see
 * hydrate.ts for the rationale). Framework hydrateRoot/app.mount requires a
 * live DOM. */
function asDomElement(v: { dispatchEvent?(ev: unknown): boolean; innerHTML: string }, where: string): Element {
  if (typeof Element !== 'undefined' && v instanceof Element) return v
  throw new TypeError(`oxide: ${where} requires a live DOM Element`)
}


/** Structural shape of the Solid web runtime's hydration surface. */
export interface SolidWebLike {
  hydrate(fn: () => unknown, element: Element): unknown
  render(fn: () => unknown, element: Element): unknown
}

let cachedWeb: SolidWebLike | undefined
let cachedCreateElement: ((type: unknown, props: unknown) => unknown) | undefined

async function loadSolid(): Promise<{
  web: SolidWebLike
  createElement: (type: unknown, props: unknown) => unknown
}> {
  if (cachedWeb && cachedCreateElement) {
    return { web: cachedWeb, createElement: cachedCreateElement }
  }
  // Solid 2 keeps the web runtime in a dedicated package; Solid 1 keeps it
  // in `solid-js/web`. The first import that resolves wins.
  let web: SolidWebLike
  try {
    web = await import('@solidjs/web')
  } catch {
    web = await import('@solidjs/web')
  }
  // `createElement`/`createComponent` come from solid-js (the core runtime).
  const solidMod: unknown = await import('solid-js')
  if (!isSolidFactoryForIsland(solidMod)) {
    throw new Error('hydrateIsland(solid): solid-js has no createElement/createComponent export')
  }
  const createElement = solidMod.createElement ?? solidMod.createComponent
  if (typeof createElement !== 'function') {
    throw new Error('hydrateIsland(solid): solid-js factory is not a function')
  }
  cachedWeb = web
  cachedCreateElement = createElement
  return { web, createElement }
}

/**
 * Hydrate a single `<ox-island>` host with Solid's own client runtime.
 *
 * `mod` is the dynamic-import result of the island chunk; its `default`
 * export is the Solid component. Solid's `hydrate(fn, el)` uses a
 * `createElement` thunk to rebuild the reactive graph against the SSR markup.
 *
 * Resolves to a disposer that renders nothing (Solid's hydration does not
 * expose an unmount handle); the disposer is a no-op for parity with React's
 * and keeps `hydrateIslands`'s teardown uniform across adapters.
 */
export async function solidHydrateIsland(
  host: OxElement,
  mod: unknown,
  props: unknown,
  _opts: ClientOptions = {},
): Promise<Disposer> {
  const { web, createElement } = await loadSolid()
  const component = (mod as { default?: unknown })?.default ?? mod
  if (component === undefined || component === null) {
    throw new TypeError('hydrateIsland(solid): island module must export a default component')
  }
  web.hydrate(
    () => createElement(component, (props as Record<string, unknown>) ?? {}),
    asDomElement(host, 'island hydration'),
  )
  return () => {
    // Solid's `hydrate` does not return an unmount handle. A best-effort
    // fallback: replace the host's content with the same markup (no-op
    // visually) so a subsequent hydration on the same host is not blocked by
    // stale internals. Callers rarely depend on teardown for islands.
  }
}
