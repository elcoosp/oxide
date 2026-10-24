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

import type { Disposer } from '../../core/src/types.ts'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Structural shape of the Svelte 5 client `mount` / `unmount` surface. */
export interface SvelteLike {
  mount(
    component: unknown,
    opts: { target: Element; props?: Record<string, unknown>; hydrate?: boolean },
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
 * export is the Svelte component. `mount(comp, { target, props, hydrate:
 * true })` hydrates the existing SSR markup instead of replacing it.
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
  const mounted = svelte.mount(component, {
    target: host as unknown as Element,
    props: (props as Record<string, unknown>) ?? {},
    hydrate: true,
  })
  return () => {
    try {
      svelte.unmount(mounted)
    } catch {
      // unmount throws if the host was already detached (route swap); a
      // best-effort cleanup is enough.
    }
  }
}
