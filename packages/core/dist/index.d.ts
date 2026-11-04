import { fromBase64, toBase64 } from "./bytes.js";
import { a as FrameworkAdapter, c as Plugin, i as Disposer, l as RenderCtx, n as ClientManifest, o as HeadTags, r as ClientRouteEntry, s as PageProps, t as ClientInput, u as RenderInput } from "./types-CoCogOdY.js";
import { boot } from "./client-boot.js";
import { DeferredClient, SwapDetail, attachDeferGlue, wireDeferreds } from "./client-defer-glue.js";
import { EnhanceForm, EnhanceOptions, enhance } from "./client-enhance.js";
import { Router, createRouter } from "./client-router.js";
import { IslandHost, IslandsAdapter, IslandsOptions, decodeIslandProps, hydrateIsland, hydrateIslands, parseStrategy } from "./client-islands.js";
import { a as VitalSample, d as OdfError, f as decodeOdf, i as scheduleScript, n as ScriptStrategy, o as VitalsOptions, p as decodeOdfToJson, r as loadScripts, s as reportWebVitals, t as ScriptOptions } from "./scripts--om8cvP3.js";
import { __oxide_call_fn } from "./fn.js";
import { TestApp, TestAppRequest, TestHandler, createTestApp } from "./test.js";
/** The config object shape `oxide.config.{ts,mjs,js}` exports. Kept
 *  permissive because the Rust-side schema is the source of truth; the
 *  TS type exists for editor ergonomics. Fields are optional and unknown
 *  extras are allowed. */
interface OxideUserConfig {
  framework?: unknown;
  routes?: {
    dir?: string;
    [k: string]: unknown;
  };
  server?: Record<string, unknown>;
  render?: Record<string, unknown>;
  build?: Record<string, unknown>;
  adapter?: string;
  modules?: unknown[];
  [k: string]: unknown;
}
/** Identity helper that anchors the config type in editors. */
export declare function defineConfig<T extends OxideUserConfig>(config: T): T;
export { ClientInput, ClientManifest, ClientRouteEntry, DeferredClient, Disposer, type EnhanceForm, type EnhanceOptions, FrameworkAdapter, HeadTags, type IslandHost, type IslandsAdapter, type IslandsOptions, OdfError, type OxideUserConfig, PageProps, Plugin, RenderCtx, RenderInput, type Router, type ScriptOptions, type ScriptStrategy, type SwapDetail, type TestApp, type TestAppRequest, type TestHandler, type VitalSample, type VitalsOptions, __oxide_call_fn, attachDeferGlue, boot, createRouter, createTestApp, decodeIslandProps, decodeOdf, decodeOdfToJson, enhance, fromBase64, hydrateIsland, hydrateIslands, loadScripts, parseStrategy, reportWebVitals, scheduleScript, toBase64, wireDeferreds };
//# sourceMappingURL=index.d.ts.map