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
import { hydrate, hydrateIsland as irHydrateIsland, mount, update, type ClientOptions } from './hydrate.ts'
import { reactHydrateIsland } from './hydrate-island-real.ts'
import { reactRenderLoading } from './render-loading.ts'
import { createRealAdapter, type RealAdapterOptions } from './real-adapter.ts'

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
  // §7: the named `adapter` export is the neutral-IR adapter. Its
  // `hydrateIsland` replays deterministic markup against the hand-rolled
  // fake DOM (see packages/react/test/helpers), which the §22 conformance
  // suite drives. Real React hydration lives on `clientAdapter` below —
  // `hydrateRoot` needs a live DOM and would crash on the fake one.
  hydrateIsland: irHydrateIsland as unknown as FrameworkAdapter['hydrateIsland'],
  mount: mount as (root: Element, input: ClientInput) => Disposer,
  update,
}

/**
 * `clientAdapter` — the browser-side adapter for the island boot entry.
 *
 * Same manifest as the named `adapter`, but `hydrateIsland` is React's own
 * `hydrateRoot` so `useState`, event handlers and refs work. The generated
 * boot entry (`<out_dir>/.oxide/island-boot.mjs`) imports this, not the
 * neutral-IR `adapter`, so the two have distinct, well-named roles:
 *
 *   * `adapter`      → tests, conformance, environments without React
 *   * `clientAdapter` → production browser hydration
 *
 * `react-dom/client` is code-split inside `reactHydrateIsland`, so a page
 * with no islands never fetches it.
 */
export const clientAdapter: FrameworkAdapter = {
  ...adapter,
  hydrateIsland: reactHydrateIsland as unknown as FrameworkAdapter['hydrateIsland'],
  renderLoading: reactRenderLoading as unknown as FrameworkAdapter['renderLoading'],
}

// ---------------------------------------------------------------------------
// Config surface (spec §14): a callable default export so oxide.config.ts can
// write `framework: react({ ssr: 'real' })`. Components come from the runtime
// resolver (§6.4), not from the config. With no options, react() returns the
// IR adapter (the same object as the named `adapter` export), so the config
// surface is a pure opt-in.
// ---------------------------------------------------------------------------

/** Options accepted by the adapter factory from oxide.config.ts. */
export interface AdapterOptions extends Partial<RealAdapterOptions> {
  /**
   * §14.1: the default is real SSR. \`ssr: 'real'\` requires it; \`'auto'\`
   * (the internal strategy value) prefers it and falls back to the IR
   * renderer when the framework deps are unavailable; \`'ir'\` is the
   * explicit escape hatch — the pre-§14.1 behavior, useful when an app
   * ships no framework runtime and only needs the meta-framework's IR
   * placeholders.
   */
  ssr?: 'real' | 'auto' | 'ir'
  /** Reserved: build-time compiler flags (currently informational). */
  compiler?: boolean | Record<string, unknown>
}

/**
 * Build a {@link FrameworkAdapter} (§7). Real upstream SSR is selected by
 * `ssr: 'real'` (or `'auto'` with an explicit `componentFor` override); the
 * route components themselves are supplied by the runtime resolver built
 * from the generated `dist/server/ssr/index.js` (§6.4). A config therefore
 * needs no component imports — `framework: react({ ssr: 'real' })`.
 */
export function createAdapter(opts: AdapterOptions = {}): FrameworkAdapter {
  // §14.1: the default is real SSR. Before this change, \`react({compiler:
  // true})\` — the literal spec example — returned the IR adapter because
  // the outer guard only accepted explicit \`ssr\` values. Now the guard is
  // inverted: everything except the explicit \`'ir'\` escape hatch routes
  // through \`createRealAdapter\` (with the internal strategy defaulting to
  // \`'auto'\`, i.e. real SSR with IR fallback when framework deps are
  // absent — the honest behavior the spec's "framework-agnostic" pitch
  // needs).
  if (opts.ssr !== 'ir') {
    return createRealAdapter({
      ssr: opts.ssr ?? 'auto',
      ...(typeof opts.componentFor === 'function' ? { componentFor: opts.componentFor } : {}),
      layoutFor: opts.layoutFor,
    })
  }
  return adapter
}

/**
 * The default export is a callable factory so `oxide.config.ts` reads
 * naturally: `framework: react({ ssr: 'real' })`. Named
 * exports (`adapter`, `createAdapter`, `createRealAdapter`) remain
 * available for tests and internal use.
 */
function reactFactory(opts?: AdapterOptions): FrameworkAdapter {
  return createAdapter(opts ?? {})
}
export default reactFactory

// Re-exports for app code and the conformance suite.
export { render, renderToString, renderShellToString, head, mergeHead, applyTitleTemplate }
export { hydrate, irHydrateIsland, mount, update, type ClientOptions }
export { islandModules, pageComponent }
export { reactHydrateIsland } from './hydrate-island-real.ts'
export { Island, type IslandProps, type IslandStrategy } from './island.ts'
export { Image, type ImageProps, type ImageVariantModule } from './image.ts'
export {
  el, t, raw, frag, deferred, island, hash6, escapeText, escapeAttr,
  type Node, type ComponentFn, type DeferSource,
} from './ir.ts'
export {
  SWAPPER_SRC, boundaryFallback, boundaryContent, islandElement, errorBoundary, hydrationBootstrap,
} from './protocol.ts'
