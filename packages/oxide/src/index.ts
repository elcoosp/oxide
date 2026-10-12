// oxide — the runtime package (spec §23 packages/oxide): server data APIs,
// adapter contract types, client runtime re-exports, and the §16.6 test
// helper. Framework adapters live in @oxide/react|vue|svelte|solid.

export * from './types.ts'
export { createTestApp, type TestApp, type TestAppRequest, type TestHandler } from './test.ts'
export { decodeOdf, decodeOdfToJson, OdfError } from './client/odf-decode.ts'
export { createRouter, type Router } from './client/router.ts'
export {
  hydrateIslands, hydrateIsland, decodeIslandProps, parseStrategy,
  type IslandHost, type IslandsAdapter, type IslandsOptions,
} from './client/islands.ts'
export { attachDeferGlue, wireDeferreds, DeferredClient, type SwapDetail } from './client/defer-glue.ts'
export { enhance, type EnhanceForm, type EnhanceOptions } from './client/enhance.ts'
