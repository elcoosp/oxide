import "./types.js";
import { createTestApp } from "./test.js";
import { a as decodeOdf, i as OdfError, o as decodeOdfToJson } from "./odf-decode-D1PLHGuy.js";
import { createRouter } from "./client-router.js";
import { decodeIslandProps, hydrateIsland, hydrateIslands, parseStrategy } from "./client-islands.js";
import { DeferredClient, attachDeferGlue, wireDeferreds } from "./client-defer-glue.js";
import { enhance } from "./client-enhance.js";
import { fromBase64, toBase64 } from "./bytes.js";
import { n as scheduleScript, t as loadScripts } from "./scripts-BUPfxzLn.js";
import { boot } from "./client-boot.js";
import { t as reportWebVitals } from "./vitals-L5C-0ePE.js";
import { __oxide_call_fn } from "./fn.js";
// src/config.ts
/** Identity helper that anchors the config type in editors. */
function defineConfig(config) {
	return config;
}
export { DeferredClient, OdfError, __oxide_call_fn, attachDeferGlue, boot, createRouter, createTestApp, decodeIslandProps, decodeOdf, decodeOdfToJson, defineConfig, enhance, fromBase64, hydrateIsland, hydrateIslands, loadScripts, parseStrategy, reportWebVitals, scheduleScript, toBase64, wireDeferreds };

//# sourceMappingURL=index.js.map