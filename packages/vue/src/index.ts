// @oxide/vue — the adapter manifest + runtime entries (spec §7, contract 1).
//
// The adapter owns components, rendering, hydration and HMR; the core owns
// routes, loaders, actions, cache, head merging, serialization, middleware,
// client navigation, assets and the streaming protocol.
//
// Capabilities (§7): suspense 'core' (Vue's streaming is partial — the core
// owns the deferred boundaries via the §7.1 protocol), islands true (§10.3
// hydrateIsland), HMR 'component' (SFC-scoped reload, §13.2).

import type { FrameworkAdapter, RenderInput, RenderCtx, HeadTags, ClientInput, Disposer } from '../../oxide/src/types.ts'
import type { Plugin } from '../../oxide/src/types.ts'
import { render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate, islandModules, pageComponent } from './render.ts'
import { hydrate, hydrateIsland, mount, update, type ClientOptions } from './hydrate.ts'
import { createRealAdapter, type RealAdapterOptions } from './real-adapter.ts'

export const adapter: FrameworkAdapter = {
  contract: 1,
  name: 'vue',
  extensions: ['.vue'],
  capabilities: {
    suspense: 'core',
    islands: true,
    hmr: 'component',
  },
  plugins(opts: unknown): Plugin[] {
    // Framework compile plugins for the tooling pool (§15.2): SFC transform +
    // component-scoped HMR. Runtime-agnostic shapes (no deps installed here).
    void opts
    return [
      { name: 'oxide:vue-sfc', enforce: 'pre', transform: 'sfc' },
      { name: 'oxide:vue-hmr', enforce: 'post', hmr: 'component' },
    ]
  },
  render,
  head,
  hydrate: hydrate as (root: Element, input: ClientInput) => Disposer,
  hydrateIsland: hydrateIsland as unknown as FrameworkAdapter['hydrateIsland'],
  mount: mount as (root: Element, input: ClientInput) => Disposer,
  update,
}

import { vueHydrateIsland } from './hydrate-island-real.ts'
import { vueRenderLoading } from './render-loading.ts'

export const clientAdapter: FrameworkAdapter = {
  ...adapter,
  // §10.3: real `createSSRApp().mount()` hydration so Vue reactivity works.
  hydrateIsland: vueHydrateIsland as never,
  // §8.4: real `createApp().mount()` for per-route loading boundaries.
  renderLoading: vueRenderLoading as never,
}

// ---------------------------------------------------------------------------
// Config surface (spec §14): a callable default export so oxide.config.ts can
// write `framework: vue({ ssr: 'real', componentFor })`. With no component
// resolver supplied, vue() returns the IR adapter (the same object as the
// named `adapter` export), so the config surface is a pure opt-in.
// ---------------------------------------------------------------------------

/** Options accepted by the adapter factory from oxide.config.ts. */
export interface AdapterOptions extends Partial<RealAdapterOptions> {
  /** Reserved: build-time compiler flags (currently informational). */
  compiler?: boolean | Record<string, unknown>
}

/**
 * Build a {@link FrameworkAdapter}. With `componentFor` set, the adapter
 * dispatches `render()` to the real upstream renderer (`react-dom/server`,
 * `@vue/server-renderer`, `svelte/server`, `@solidjs/web`) for every route
 * the resolver returns a component for, falling back to the neutral-IR
 * renderer when `ssr: 'auto'`. Without `componentFor`, the IR adapter is
 * returned unchanged.
 */
export function createAdapter(opts: AdapterOptions = {}): FrameworkAdapter {
  if (typeof opts.componentFor === 'function') {
    return createRealAdapter({
      ssr: opts.ssr ?? 'auto',
      componentFor: opts.componentFor,
      layoutFor: opts.layoutFor,
    })
  }
  return adapter
}

/**
 * The default export is a callable factory so `oxide.config.ts` reads
 * naturally: `framework: vue({ ssr: 'real', componentFor })`. Named
 * exports (`adapter`, `createAdapter`, `createRealAdapter`) remain
 * available for tests and internal use.
 */
function vueFactory(opts?: AdapterOptions): FrameworkAdapter {
  return createAdapter(opts ?? {})
}
export default vueFactory

// Re-exports for app code and the conformance suite.
export { render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate }
export { hydrate, hydrateIsland, mount, update, type ClientOptions }
export { islandModules, pageComponent }
export {
  el, t, raw, frag, deferred, island, hash6, escapeText, escapeAttr,
  type Node, type ComponentFn, type DeferSource,
} from './ir.ts'
export {
  SWAPPER_SRC, boundaryFallback, boundaryContent, islandElement, errorBoundary, hydrationBootstrap,
} from './protocol.ts'
