// Client runtime entry (spec §8.4, §10.3): router, islands loader, defer
// glue, form enhance, and the ODF decoder the browser bundles (~1 KB target).

export { createRouter, type Router, type RouterDoc, type RouterLink, type RouterOptions } from './router.ts'
export {
  hydrateIslands, hydrateIsland, decodeIslandProps, parseStrategy,
  type IslandCtx, type IslandHost, type IslandsAdapter, type IslandsOptions, type ParsedStrategy,
} from './islands.ts'
export {
  attachDeferGlue, wireDeferreds, DeferredClient,
  type DeferGlue, type DeferGlueOptions, type SwapDetail, type WireOptions,
} from './defer-glue.ts'
export { enhance, type EnhanceForm, type EnhanceOptions, type EnhanceSubmitEvent } from './enhance.ts'
// §V2-F07: the resumable-fetch lifecycle — the router's data fetch rides it,
// and app code calling a streamed `route.ts` endpoint marked
// `resumable(...)` imports it here.
export { resumableFetch, type ResumableFetchOptions } from './resume.ts'
export {
  decodeOdf, decodeOdfToJson, decodeOdfOrPatch, applyPatch, sha256Hex16,
  OdfError, MAX_BYTES, MAX_DEPTH, MAX_REFS,
} from './odf-decode.ts'
export { reportWebVitals, type VitalSample, type VitalsOptions } from './vitals.ts'
export { loadMessages, setMessageLocales, t, type Messages } from './messages.ts'
export { scheduleScript, loadScripts, type ScriptStrategy, type ScriptOptions } from './scripts.ts'
export {
  componentFor, routeIdFor, pageComponentExport,
  setRouteComponentSource, getRouteComponentSource, isParamSegment,
  type ClientManifestLike, type RouteComponentSource, type RouteManifestEntry,
} from './route-manifest.ts'
export * from './search.ts'
export {
  search,
  tokenize,
  type SearchHit,
  type SearchResponse,
  type SearchOptions,
  type SearchMode,
  type StaticIndex,
  type StaticDoc,
} from './searchEngine.ts'
// V2-E02: typed search params + the runtime of the generated href overloads.
export {
  parseSearch,
  type SearchParseResult,
  type SearchParseSuccess,
  type SearchParseFailure,
  type StandardIssue,
  type StandardSchemaV1Like,
} from './search-params.ts'
export {
  hrefFor, goto,
  type HrefOpts, type GotoOpts, type GotoRouter, type HrefParam,
} from './href.ts'
// §V2-E04: the realm's Trusted Types policy — importing this module is
// the "defined once" statement; sinks route through `tt.createHTML`.
export { tt, type OxideTrustedTypesPolicy } from './tt.ts'

// Phase 1 (Native ESM dev server): re-export the HMR runtime's public type.
// The `hot.ts` module side-effect (`install()`) is guarded so importing it
// is a no-op outside a browser. Type-only re-export means `client.js`
// never imports `client-hot.js` at runtime.
export { type HotContext } from './hot.ts'
