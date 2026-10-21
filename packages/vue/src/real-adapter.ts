// @oxide/vue — `createRealAdapter` (spec §7): a FrameworkAdapter whose
// `render()` dispatches to the REAL `@vue/server-renderer` for any route the
// caller resolves a Vue component for. The default adapter (./index.ts)
// remains the neutral-IR renderer that drives the §22 conformance suite; this
// factory is the production path, exercised end-to-end by ./real-ssr.ts.
//
// Usage:
//
//   import { createRealAdapter } from '@oxide/vue/real-adapter'
//   const adapter = createRealAdapter({
//     ssr: 'auto',
//     componentFor: (route) => routes[route]?.component,
//   })
//
// Head merge order (spec §16.3, matching the IR renderer's buildShell):
//     mergeHead( mergeHead( LAYOUT_HEAD, input.head ), head(ctx) )

import { makeRealAdapter, type RealAdapterOptions } from '../../oxide/src/server/real-ssr.ts'
import type { RenderInput, HeadTags, RenderCtx } from '../../oxide/src/types.ts'
import { render as irRender, head as irHead, mergeHead } from './render.ts'
import { hydrate, hydrateIsland, mount, update } from './hydrate.ts'
import { vueHydrateIsland } from './hydrate-island-real.ts'
import { isVueAvailable, renderVueToString, renderVueToStream, getLastLoadError } from './real-ssr.ts'

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

function buildHead(input: RenderInput): HeadTags {
  const ctx: RenderCtx = {
    route: input.route, url: input.url, params: input.params,
    data: input.data, locals: input.locals,
  }
  return mergeHead(mergeHead(LAYOUT_HEAD, input.head), irHead(ctx))
}

/**
 * Build a Vue adapter. Components come from the runtime resolver
 * (`RenderInput.resolve`, §6.4): an SFC output, `h()`-built object, or
 * `defineComponent` result. An explicit `componentFor` overrides it. When
 * `ssr` is 'auto' (default), a missing component or missing
 * `vue`/`@vue/server-renderer` deps falls back to the neutral-IR renderer;
 * when 'real', a missing page is the §10.2 OX-500 diagnostic.
 */
export function createRealAdapter(opts: RealAdapterOptions) {
  return makeRealAdapter({
    name: 'vue',
    extensions: ['.vue'],
    capabilities: { suspense: 'core', islands: true, hmr: 'component' },
    plugins: () => [
      { name: 'oxide:vue-sfc', enforce: 'pre', transform: 'sfc' },
      { name: 'oxide:vue-hmr', enforce: 'post', hmr: 'component' },
    ],
    irRender,
    buildHead,
    isAvailable: isVueAvailable,
    lastError: getLastLoadError,
    renderBody: renderVueToString,
    // §7.1: Vue's real streaming SSR via `@vue/server-renderer`'s renderToStream.
    renderStream: renderVueToStream,
    rootAttrs: ' id="root" data-v-app=""',
    hydrate: hydrate as never,
    // §10.3: real `createSSRApp().mount()` so Vue reactivity works, not the
    // IR replay that only handles static markup.
    hydrateIsland: vueHydrateIsland as never,
    mount: mount as never,
    update,
    opts,
  })
}
