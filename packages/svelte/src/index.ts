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

export default adapter

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
