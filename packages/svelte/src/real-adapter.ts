// @oxide/svelte — `createRealAdapter` (spec §7): a FrameworkAdapter whose
// `render()` dispatches to the REAL `svelte/server` renderer for any route
// the caller resolves a compiled Svelte component for. The default adapter
// (./index.ts) remains the neutral-IR renderer that drives the §22
// conformance suite; this factory is the production path.

import { makeRealAdapter, type RealAdapterOptions } from '../../oxide/src/server/real-ssr.ts'
import type { RenderInput, HeadTags, RenderCtx } from '../../oxide/src/types.ts'
import { render as irRender, head as irHead, mergeHead } from './render.ts'
import { hydrate, hydrateIsland, mount, update } from './hydrate.ts'
import { svelteHydrateIsland } from './hydrate-island-real.ts'
import { isSvelteAvailable, renderSvelteToString, getLastLoadError } from './real-ssr.ts'

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
 * Build a Svelte adapter. Components come from the runtime resolver
 * (`RenderInput.resolve`, §6.4): the default export of a compiled SFC. An
 * explicit `componentFor` overrides it. When `ssr` is 'auto' (default), a
 * missing component or missing `svelte` dep falls back to the neutral-IR
 * renderer; when 'real', a missing page is the §10.2 OX-500 diagnostic.
 */
export function createRealAdapter(opts: RealAdapterOptions) {
  return makeRealAdapter({
    name: 'svelte',
    extensions: ['.svelte'],
    capabilities: { suspense: 'core', islands: true, hmr: 'component' },
    plugins: () => [
      { name: 'oxide:svelte-compile', enforce: 'pre', transform: 'svelte' },
      { name: 'oxide:svelte-hmr', enforce: 'post', hmr: 'component' },
    ],
    irRender,
    buildHead,
    isAvailable: isSvelteAvailable,
    lastError: getLastLoadError,
    renderBody: renderSvelteToString,
    rootAttrs: ' id="root" class="oxide-root"',
    hydrate: hydrate as never,
    // §10.3: real `mount(..., { hydrate: true })` so Svelte 5 runes work.
    hydrateIsland: svelteHydrateIsland as never,
    mount: mount as never,
    update,
    opts,
  })
}
