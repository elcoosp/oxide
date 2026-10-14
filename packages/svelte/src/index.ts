// @oxide/svelte — the adapter manifest + runtime entries (spec §7, contract 1).
//
// The adapter owns components, rendering, hydration and HMR; the core owns
// routes, loaders, actions, cache, head merging, serialization, middleware,
// client navigation, assets and the streaming protocol.
//
// Capabilities (§7): suspense 'core' (Svelte renders synchronously per tree —
// the core owns the deferred boundaries via the §7.1 protocol), islands true
// (§10.3 hydrateIsland), HMR 'component' (SFC-scoped reload, §13.2).

import type { FrameworkAdapter, RenderInput, RenderCtx, HeadTags, ClientInput, Disposer } from '../../oxide/src/types.ts'
import type { Plugin } from '../../oxide/src/types.ts'
import { render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate, islandModules, pageComponent } from './render.ts'
import { hydrate, hydrateIsland, mount, update, type ClientOptions } from './hydrate.ts'
import { createRealAdapter, type RealAdapterOptions } from './real-adapter.ts'

export const adapter: FrameworkAdapter = {
  contract: 1,
  name: 'svelte',
  extensions: ['.svelte'],
  capabilities: {
    suspense: 'core',
    islands: true,
    hmr: 'component',
  },
  plugins(opts: unknown): Plugin[] {
    // Framework compile plugins for the tooling pool (§15.2): Svelte compile +
    // component-scoped HMR. Runtime-agnostic shapes (no deps installed here).
    void opts
    return [
      { name: 'oxide:svelte-compile', enforce: 'pre', transform: 'svelte' },
      { name: 'oxide:svelte-hmr', enforce: 'post', hmr: 'component' },
    ]
  },
  render,
  head,
  hydrate: hydrate as (root: Element, input: ClientInput) => Disposer,
  hydrateIsland: hydrateIsland as unknown as FrameworkAdapter['hydrateIsland'],
  mount: mount as (root: Element, input: ClientInput) => Disposer,
  update,
}

/**
 * `clientAdapter` — the browser-side adapter for the island boot entry.
 *
 * For svelte this is currently the same object as the named `adapter`
 * (the framework's client hydration path is the IR replay). When
 * svelte's real client-side hydration is wired (parallel to React's
 * `react-dom/client` path), it overrides `hydrateIsland` here and the
 * generated boot entry picks it up with no CLI change.
 */
export const clientAdapter: FrameworkAdapter = adapter

// ---------------------------------------------------------------------------
// Config surface (spec §14): a callable default export so oxide.config.ts can
// write `framework: svelte({ ssr: 'real', componentFor })`. With no component
// resolver supplied, svelte() returns the IR adapter (the same object as the
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
 * naturally: `framework: svelte({ ssr: 'real', componentFor })`. Named
 * exports (`adapter`, `createAdapter`, `createRealAdapter`) remain
 * available for tests and internal use.
 */
function svelteFactory(opts?: AdapterOptions): FrameworkAdapter {
  return createAdapter(opts ?? {})
}
export default svelteFactory

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
