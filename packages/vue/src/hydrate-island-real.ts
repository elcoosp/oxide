// @oxide/vue real island hydration via `@vue/runtime-dom`'s `createSSRApp`.
//
// The IR-based `hydrateIsland` in ./hydrate.ts replays deterministic markup
// and only works for components without a reactive lifecycle — any real app
// needs Vue's own hydration so `ref`/`reactive`/`watch`/`onMounted` and
// component-scoped state survive.
//
// `vue` is loaded lazily: an island page loads it when the first island's
// strategy fires (typically `load`, or on idle / intersection for the other
// strategies). A page with no islands never fetches it.
//
// The returned value is a `Promise<Disposer>`; `packages/oxide/src/client/
// islands.ts` awaits it before marking the host hydrated.

import type { Disposer } from '@oxide-fw/core/types'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Return the module's `default` export when an interop wrapper provides
 * one; otherwise the module itself. Bundlers targeting older CJS produce
 * `{ default: ... }` wrappers; native ESM does not. This helper extracts
 * the right value without a cast — the runtime check is what determines
 * which shape is present. */
function pickInteropDefault(mod: unknown): unknown {
  if (mod !== null && typeof mod === 'object' && 'default' in mod) {
    const d = (mod as { default: unknown }).default
    if (d !== undefined) return d
  }
  return mod
}

/** Assert that `v` has `createSSRApp` (what a `VueLike` needs at runtime). */
function assertVueLike(v: unknown): VueLike {
  if (v === null || typeof v !== 'object') {
    throw new TypeError('vue: module is not an object')
  }
  const candidate = v as { createSSRApp?: unknown }
  if (typeof candidate.createSSRApp !== 'function') {
    throw new TypeError('vue: module has no createSSRApp export')
  }
  // The runtime check above is what VueLike requires. The remaining fields
  // are typed by the interface but not verified here — Vue's own type
  // guarantees them once createSSRApp exists.
  return v as VueLike
}


/** Narrow an `OxElement` to a real DOM `Element` (runtime assertion — see
 * hydrate.ts for the rationale). Framework hydrateRoot/app.mount requires a
 * live DOM. */
function asDomElement(v: { dispatchEvent?(ev: unknown): boolean; innerHTML: string }, where: string): Element {
  if (typeof Element !== 'undefined' && v instanceof Element) return v
  throw new TypeError(`oxide: ${where} requires a live DOM Element`)
}


/** Structural shape of the `vue` module's SSR-app surface. */
export interface VueLike {
  createSSRApp(component: unknown, props: Record<string, unknown>): {
    mount(container: Element | string): unknown
    unmount(): void
  }
}

let cachedVue: VueLike | undefined

async function loadVue(): Promise<VueLike> {
  if (cachedVue) return cachedVue
  // A dynamic import so the runtime is code-split into a chunk the browser
  // only fetches when the first island's strategy fires.
  const mod: unknown = await import('vue')
  cachedVue = assertVueLike(pickInteropDefault(mod))
  return cachedVue
}

/**
 * Hydrate a single `<ox-island>` host with Vue's own client runtime.
 *
 * `mod` is the dynamic-import result of the island chunk; its `default`
 * export is the Vue component (a plain options object, a `defineComponent`
 * result, or an SFC-compiled component). Vue's `createSSRApp().mount()`
 * hydrates the existing SSR markup instead of replacing it, so the initial
 * state the server produced stays put until the user interacts.
 *
 * Resolves to a disposer that unmounts the Vue app; `hydrateIslands` stores
 * it and calls it on page teardown.
 */
export async function vueHydrateIsland(
  host: OxElement,
  mod: unknown,
  props: unknown,
  _opts: ClientOptions = {},
): Promise<Disposer> {
  const vue = await loadVue()
  const component = (mod as { default?: unknown })?.default ?? mod
  if (component === undefined || component === null) {
    throw new TypeError('hydrateIsland(vue): island module must export a default component')
  }
  const app = vue.createSSRApp(component, (props as Record<string, unknown>) ?? {})
  app.mount(asDomElement(host, 'island hydration'))
  return () => {
    try {
      app.unmount()
    } catch {
      // unmount throws if the host was already detached (route swap); a
      // best-effort cleanup is enough.
    }
  }
}
