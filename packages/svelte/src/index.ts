// @oxide/svelte — the adapter manifest + runtime entries (spec §7, contract 1).
//
// The adapter owns components, rendering, hydration and HMR; the core owns
// routes, loaders, actions, cache, head merging, serialization, middleware,
// client navigation, assets and the streaming protocol.
//
// Capabilities (§7): suspense 'core' (Svelte renders synchronously per tree —
// the core owns the deferred boundaries via the §7.1 protocol), islands true
// (§10.3 hydrateIsland), HMR 'component' (SFC-scoped reload, §13.2).

import type { FrameworkAdapter, RenderInput, RenderCtx, HeadTags, ClientInput, Disposer } from '../../core/src/types.ts'
import type { Plugin } from '../../core/src/types.ts'
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
import { svelteHydrateIsland } from './hydrate-island-real.ts'
import { svelteRenderLoading } from './render-loading.ts'

export const clientAdapter: FrameworkAdapter = {
  ...adapter,
  // §10.3: real `mount(..., { hydrate: true })` hydration so Svelte 5 runes work.
  hydrateIsland: svelteHydrateIsland as never,
  // §8.4: real `mount(...)` for per-route loading boundaries.
  renderLoading: svelteRenderLoading as never,
}

// ---------------------------------------------------------------------------
// Config surface (spec §14): a callable default export so oxide.config.ts can
// write `framework: svelte({ ssr: 'real', componentFor })`. With no component
// resolver supplied, svelte() returns the IR adapter (the same object as the
// named `adapter` export), so the config surface is a pure opt-in.
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
