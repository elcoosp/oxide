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

import { makeRealAdapter, type RealAdapterOptions } from '../../oxide/src/server/real-ssr.ts'
import type { RenderInput, HeadTags, RenderCtx } from '../../oxide/src/types.ts'
import { render as irRender, head as irHead, mergeHead } from './render.ts'
import { hydrate, hydrateIsland, mount, update } from './hydrate.ts'
import { solidHydrateIsland } from './hydrate-island-real.ts'
import { isSolidAvailable, renderSolidToString, getLastLoadError } from './real-ssr.ts'

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
 * Build a Solid adapter. `componentFor(route)` MUST return a Solid component
 * (a function of props) or undefined. When `ssr` is 'auto' (default), a
 * missing component or missing `solid-js` dep falls back to the neutral-IR
 * renderer; when 'real', the failure is emitted as an HTML comment.
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
    // Solid's renderer takes a zero-argument factory. Pre-invoke the
    // component OUTSIDE solid's async machinery so a synchronous throw
    // propagates immediately (otherwise renderToStringAsync hangs and
    // reports "renderToString timed out"). The built tree is then handed
    // back to solid as a stable value.
    renderBody: async (Component, props) => {
      if (typeof Component !== 'function') return String(Component ?? '')
      // Solid's renderer takes a zero-argument factory. Pre-invoke the
      // component OUTSIDE solid's async machinery so a synchronous throw
      // propagates immediately (otherwise renderToStringAsync hangs and
      // reports "renderToString timed out"). The built tree is then handed
      // back to solid as a stable value.
      const tree = (Component as (p: Record<string, unknown>) => unknown)(props)
      return renderSolidToString(() => tree)
    },
    rootAttrs: ' id="root" data-solid=""',
    hydrate: hydrate as never,
    // §10.3: real `hydrate()` so Solid signals work.
    hydrateIsland: solidHydrateIsland as never,
    mount: mount as never,
    update,
    opts,
  })
}
