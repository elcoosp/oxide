// oxide — the runtime package (spec §23 packages/oxide): server data APIs,
// adapter contract types, client runtime re-exports, and the §16.6 test
// helper. Framework adapters live in @oxide/react|vue|svelte|solid.

export * from './types.ts'
export { defineConfig, type OxideUserConfig } from './config.ts'
export { createTestApp, type TestApp, type TestAppRequest, type TestHandler } from './test.ts'
export { decodeOdf, decodeOdfToJson, OdfError } from './client/odf-decode.ts'
export { createRouter, type Router } from './client/router.ts'
export {
  hydrateIslands, hydrateIsland, decodeIslandProps, parseStrategy,
  type IslandHost, type IslandsAdapter, type IslandsOptions,
} from './client/islands.ts'
export { attachDeferGlue, wireDeferreds, DeferredClient, type SwapDetail } from './client/defer-glue.ts'
export { enhance, type EnhanceForm, type EnhanceOptions } from './client/enhance.ts'
// §V2-F07: the resumable-fetch lifecycle (the router's data fetch rides it;
// app code calling a streamed, `resumable(...)`-marked endpoint imports it).
export { resumableFetch, type ResumableFetchOptions } from './client/resume.ts'
export { toBase64, fromBase64 } from './bytes.ts'
export { boot } from './client/boot.ts'
export { scheduleScript, loadScripts, type ScriptStrategy, type ScriptOptions } from './client/scripts.ts'
// §V2-F17: the component-cost dev overlay (the client half the server's
// `/__oxide/cost` wire feeds). Exported for hosts that mount it themselves —
// the boot mounts it on the ESM dev path automatically.
export { mountCostOverlay, type CostOverlayOptions } from './client/cost-overlay.ts'
export { reportWebVitals, type VitalSample, type VitalsOptions } from './client/vitals.ts'
export { __oxide_call_fn } from './fn.ts'
// §V2-E06 Module API v1 — `defineModule` + `ModuleContext` (the authoring
// surface; the Rust `modules_loader` consumes the same `oxide.module`
// contract). One contract, not two.
export {
  defineModule,
  OxideModuleError,
  type ModuleAdapter,
  type ModuleContext,
  type ModuleDefinition,
  type ModuleHost,
  type ModuleManifest,
  type ModuleMiddleware,
  type ModulePlugin,
  type ModuleRoute,
  type ModuleServerFn,
  type ModuleTypes,
} from './module.ts'
