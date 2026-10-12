// @oxide/react — the adapter manifest + runtime entries (spec §7, contract 1).
//
// The adapter owns components, rendering, hydration and HMR; the core owns
// routes, loaders, actions, cache, head merging, serialization, middleware,
// client navigation, assets and the streaming protocol.
//
// Capabilities (§7): suspense 'native' (React owns streaming boundaries — the
// IR Defer node still emits the core `template[data-ox-b]` wire shapes so the
// §22 conformance suite is identical across adapters), islands true
// (§10.3 hydrateIsland), HMR 'refresh' (react-refresh, §13.2).

import type { FrameworkAdapter, RenderInput, RenderCtx, HeadTags, ClientInput, Disposer } from '../../oxide/src/types.ts'
import type { Plugin } from '../../oxide/src/types.ts'
import { render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate, islandModules, pageComponent } from './render.ts'
import { hydrate, hydrateIsland, mount, update, type ClientOptions } from './hydrate.ts'

export const adapter: FrameworkAdapter = {
  contract: 1,
  name: 'react',
  extensions: ['.tsx'],
  jsxImportSource: 'react',
  capabilities: {
    suspense: 'native',
    islands: true,
    hmr: 'refresh',
  },
  plugins(opts: unknown): Plugin[] {
    // Framework compile plugins for the tooling pool (§15.2): JSX transform +
    // react-refresh HMR. Runtime-agnostic shapes (no deps installed here).
    void opts
    return [
      { name: 'oxide:react-jsx', enforce: 'pre', transform: 'jsx', jsxImportSource: 'react' },
      { name: 'oxide:react-refresh', enforce: 'post', hmr: 'refresh' },
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
