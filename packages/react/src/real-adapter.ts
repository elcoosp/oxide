// @oxide/react — `createRealAdapter` (spec §7): a FrameworkAdapter whose
// `render()` dispatches to the REAL `react-dom/server` renderer for any route
// the caller resolves a React component for. The default adapter (./index.ts)
// remains the neutral-IR renderer that drives the §22 conformance suite; this
// factory is the production path, exercised end-to-end by ./real-ssr.ts.
//
// Usage:
//
//   import { createRealAdapter } from '@oxide/react/real-adapter'
//   const adapter = createRealAdapter({
//     ssr: 'auto',
//     componentFor: (route) => routes[route]?.component,
//     layoutFor: () => AppLayout,
//   })
//
// Head merge order (spec §16.3, matching the IR renderer's buildShell):
//     mergeHead( mergeHead( LAYOUT_HEAD, input.head ), head(ctx) )
// where `head(ctx)` is derived from the input data as in the IR adapter.
// The final merged model is emitted by the shared helper.

import { makeRealAdapter, type RealAdapterOptions } from '../../oxide/src/server/real-ssr.ts'
import type { RenderInput, HeadTags, RenderCtx } from '../../oxide/src/types.ts'
import { render as irRender, head as irHead, mergeHead } from './render.ts'
import { hydrate, mount, update } from './hydrate.ts'
import { reactHydrateIsland } from './hydrate-island-real.ts'
import { isReactAvailable, renderReactToString, renderReactToStream, getLastLoadError } from './real-ssr.ts'

export type { RealAdapterOptions }

/** The adapter's own layout head (mirrors LAYOUT_HEAD in ./render.ts). */
const LAYOUT_HEAD: HeadTags = {
  title: undefined,
  titleTemplate: '%s · Atlas',
  meta: [{ charset: 'utf-8' }, { name: 'viewport', content: 'width=device-width, initial-scale=1' }],
  link: [],
  style: [],
  script: [],
}

/**
 * Compose the final HeadTags for a RenderInput. Merge order matches the IR
 * renderer: outer = adapter layout head, then the pipeline-supplied
 * `input.head` (route/loader head), then the adapter's `head(ctx)` model
 * (which the IR path derives from the same data via head(ctx)).
 */
function buildHead(input: RenderInput): HeadTags {
  const ctx: RenderCtx = {
    route: input.route,
    url: input.url,
    params: input.params,
    data: input.data,
    locals: input.locals,
  }
  return mergeHead(mergeHead(LAYOUT_HEAD, input.head), irHead(ctx))
}

/**
 * Build a react adapter. Components are resolved by the runtime
 * (`RenderInput.resolve`, §6.4) from the generated SSR entry; an explicit
 * `componentFor` option overrides it. When `ssr` is 'auto' (default), a
 * missing component or missing `react`/`react-dom` deps falls back to the
 * neutral-IR renderer; when 'real', a missing page component is the §10.2
 * OX-500 diagnostic (never a silent blank body).
 */
export function createRealAdapter(opts: RealAdapterOptions) {
  return makeRealAdapter({
    name: 'react',
    extensions: ['.tsx'],
    jsxImportSource: 'react',
    capabilities: { suspense: 'native', islands: true, hmr: 'refresh' },
    plugins: () => [
      { name: 'oxide:react-jsx', enforce: 'pre', transform: 'jsx', jsxImportSource: 'react' },
      { name: 'oxide:react-refresh', enforce: 'post', hmr: 'refresh' },
    ],
    irRender,
    buildHead,
    isAvailable: isReactAvailable,
    lastError: getLastLoadError,
    renderBody: renderReactToString,
    // §7.1: React 19's real streaming SSR. The shell reaches the client before
    // deferred boundaries resolve; the worker forwards each chunk as a raw
    // binary frame end-to-end.
    renderStream: renderReactToStream,
    rootAttrs: ' id="root" data-reactroot=""',
    hydrate: hydrate as never,
    hydrateIsland: reactHydrateIsland as never,
    mount: mount as never,
    update,
    opts,
  })
}
