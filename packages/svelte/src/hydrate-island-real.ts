// @oxide/svelte real island hydration via Svelte 5's `mount(..., { hydrate: true })`.
//
// The IR-based `hydrateIsland` in ./hydrate.ts replays deterministic markup
// and only works for components without runes — any real app needs Svelte's
// own hydration so `$state`/`$derived`/`$effect` and event handlers survive.
//
// `svelte` is loaded lazily: an island page loads it when the first island's
// strategy fires (typically `load`, or on idle / intersection for the other
// strategies). A page with no islands never fetches it.
//
// The returned value is a `Promise<Disposer>`; `packages/core/src/client/
// islands.ts` awaits it before marking the host hydrated.

import type { Disposer } from '@oxide-fw/core/types'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Narrow an `OxElement` to a real DOM `Element` (runtime assertion — see
 * hydrate.ts for the rationale). Framework hydrateRoot/app.mount requires a
 * live DOM. */
function asDomElement(v: { dispatchEvent?(ev: unknown): boolean; innerHTML: string }, where: string): Element {
  if (typeof Element !== 'undefined' && v instanceof Element) return v
  throw new TypeError(`oxide: ${where} requires a live DOM Element`)
}


/** Structural shape of the Svelte 5 client `mount` / `unmount` surface.
 * Svelte 5.35+ additionally exposes `hydrate()` as its own entry point —
 * `mount(..., { hydrate: true })` silently appends a second tree, so the
 * island path prefers `hydrate()` when the runtime provides it. */
export interface SvelteLike {
  mount(
    component: unknown,
    opts: { target: Element; props?: Record<string, unknown>; hydrate?: boolean },
  ): unknown
  hydrate?(
    component: unknown,
    opts: { target: Element; props?: Record<string, unknown> },
  ): unknown
  unmount(component: unknown): void
}

let cachedSvelte: SvelteLike | undefined

async function loadSvelte(): Promise<SvelteLike> {
  if (cachedSvelte) return cachedSvelte
  // A dynamic import so the runtime is code-split into a chunk the browser
  // only fetches when the first island's strategy fires. Svelte 5 exports
  // `mount`/`unmount` from the package root; Svelte 4's `hydrate` lives on
  // the client-component constructor and is not used here.
  const mod = await import('svelte')
  cachedSvelte = mod
  return mod
}

/**
 * Hydrate a single `<ox-island>` host with Svelte's own client runtime.
 *
 * `mod` is the dynamic-import result of the island chunk; its `default`
 * export is the Svelte component. Svelte 5 exposes hydration as its own
 * entry point — `mount()` IGNORES an unknown `hydrate` option and appends a
 * fresh tree next to the SSR markup — so `hydrate()` is used when present,
 * falling back to `mount()` on runtimes without it.
 *
 * Resolves to a disposer that unmounts the component; `hydrateIslands`
 * stores it and calls it on page teardown.
 */
export async function svelteHydrateIsland(
  host: OxElement,
  mod: unknown,
  props: unknown,
  _opts: ClientOptions = {},
): Promise<Disposer> {
  const svelte = await loadSvelte()
  const component = (mod as { default?: unknown })?.default ?? mod
  if (component === undefined || component === null) {
    throw new TypeError('hydrateIsland(svelte): island module must export a default component')
  }
  const target = asDomElement(host, 'island hydration')
  const islandProps = (props as Record<string, unknown>) ?? {}
  const mounted =
    typeof svelte.hydrate === 'function'
      ? svelte.hydrate(component, { target, props: islandProps })
      : svelte.mount(component, { target, props: islandProps })
  return () => {
    try {
      svelte.unmount(mounted)
    } catch {
      // unmount throws if the host was already detached (route swap); a
      // best-effort cleanup is enough.
    }
  }
}
