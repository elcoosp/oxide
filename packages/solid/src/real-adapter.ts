// @oxide/solid — `createRealAdapter` (spec §7): a FrameworkAdapter whose
// `render()` dispatches to the REAL `@solidjs/web` renderer (Solid 2.0;
// falls back to `solid-js/web` for Solid 1.x) for any route
// the caller resolves a Solid component for. The default adapter
// (./index.ts) remains the neutral-IR renderer that drives the §22
// conformance suite; this factory is the production path.
//
// Solid's `renderToStringAsync` takes a zero-argument factory, so
// `componentFor(route)` returns a component function (props => Node) and
// `renderBody` wraps the call with the props the helper hands it. This keeps
// the props shape (`{route,url,params,data}`) uniform with every other
// adapter while matching Solid's API.

import { makeRealAdapter, type RealAdapterOptions } from '@oxide-fw/core/server/real-ssr'
import type { RenderInput, HeadTags, RenderCtx } from '@oxide-fw/core/types'
import { render as irRender, head as irHead, mergeHead } from './render.ts'
import { hydrate, hydrateIsland, mount, update } from './hydrate.ts'
import { solidHydrateIsland } from './hydrate-island-real.ts'
import { isSolidAvailable, renderSolidToString, renderSolidToStream, expandSolidChildren, getLastLoadError } from './real-ssr.ts'

export type { RealAdapterOptions }

const LAYOUT_HEAD: HeadTags = {
  title: undefined,
  titleTemplate: '%s · Atlas',
  meta: [{ charset: 'utf-8' }, { name: 'viewport', content: 'width=device-width, initial-scale=1' }],
  link: [],
  style: [],
  script: [],
}

function buildHead(input: RenderInput): HeadTags {
  const ctx: RenderCtx = {
    route: input.route, url: input.url, params: input.params,
    data: input.data, locals: input.locals,
  }
  return mergeHead(mergeHead(LAYOUT_HEAD, input.head), irHead(ctx))
}

/**
 * Build a Solid adapter. Components come from the runtime resolver
 * (`RenderInput.resolve`, §6.4): a function of props. An explicit
 * `componentFor` overrides it. When `ssr` is 'auto' (default), a missing
 * component or missing `solid-js` dep falls back to the neutral-IR renderer;
 * when 'real', a missing page is the §10.2 OX-500 diagnostic.
 */
export function createRealAdapter(opts: RealAdapterOptions) {
  return makeRealAdapter({
    name: 'solid',
    extensions: ['.tsx'],
    jsxImportSource: '@solidjs/web',
    capabilities: { suspense: 'native', islands: true, hmr: 'component' },
    plugins: () => [
      { name: 'oxide:solid-jsx', enforce: 'pre', transform: 'jsx', jsxImportSource: '@solidjs/web' },
      { name: 'oxide:solid-hmr', enforce: 'post', hmr: 'component' },
    ],
    irRender,
    buildHead,
    isAvailable: isSolidAvailable,
    lastError: getLastLoadError,
    // Solid's renderer takes a zero-argument factory. The component must be
    // INVOKED INSIDE that factory — solid scopes hydration-key generation
    // (`ssrHydrationKey()` → `getNextContextId()`) to the render root the
    // renderer creates. A tree built OUTSIDE the root renders markup without
    // `_hk` keys (nothing for the client's `hydrate()` to claim) and, under
    // the streaming worker, throws "getNextContextId cannot be used under a
    // non-hydrating context". §8.2: a layout's `children` arrives as a
    // `{ Component, props }` descriptor — expanded into a real element
    // (createComponent + props.children) before the render, so the expansion
    // still happens outside; only the PAGE component's own invocation moved
    // inside the root.
    renderBody: async (Component, props) => {
      if (typeof Component !== 'function') return String(Component ?? '')
      const expanded = await expandSolidChildren(props)
      return renderSolidToString(() => (Component as (p: Record<string, unknown>) => unknown)(expanded))
    },
    // §7.1: Solid 2's `@solidjs/web` exposes `renderToStream`, so the shell
    // can reach the client before resources resolve. Same inside-the-root
    // invocation as `renderBody` (see its comment — the hydration-key scope
    // is the render root), with the same §8.2 descriptor expansion for
    // layout children.
    renderStream: async (Component, props) => {
      if (typeof Component !== 'function') {
        const bytes = new TextEncoder().encode(String(Component ?? ''))
        return new ReadableStream<Uint8Array>({
          start(c) {
            c.enqueue(bytes)
            c.close()
          },
        })
      }
      const expanded = await expandSolidChildren(props)
      return renderSolidToStream(() => (Component as (p: Record<string, unknown>) => unknown)(expanded))
    },
    rootAttrs: ' id="root" data-solid=""',
    hydrate: hydrate,
    // §10.3: real `hydrate()` so Solid signals work.
    hydrateIsland: solidHydrateIsland,
    mount: mount,
    update,
    opts,
  })
}
