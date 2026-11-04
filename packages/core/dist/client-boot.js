import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
import { isParamSegment, setRouteComponentSource } from "./client-route-manifest.js";
import { createRouter } from "./client-router.js";
import { hydrateIslands } from "./client-islands.js";
import { attachDeferGlue } from "./client-defer-glue.js";
import { fromBase64 } from "./bytes.js";
import { t as loadScripts } from "./scripts-BUPfxzLn.js";
import { loadMessages, setMessageLocales } from "./client-messages.js";
import { t as connectHmr } from "./client-hot.js";
// src/client/boot.ts
/** A `Window` shaped object, or `undefined` in a non-browser environment. */
function maybeWindow() {
	return globalThis.window;
}
/**
* Install the boot runtime on `window` if it isn't already there. Idempotent
* across duplicate module evaluations (bundler dedup failure, SPA reimport).
* No-op outside the browser.
*/
function ensureInstalled(w) {
	if (w.__oxideRegisterAdapter) return;
	w.__oxideBoot = { loadChunk: async (chunk) => {
		try {
			return await import(
				/* @vite-ignore */
				chunk
);
		} catch (e) {
			console.error?.("[oxide] chunk load failed; deferred", chunk, e);
			return;
		}
	} };
	w.__oxideRegisterAdapter = (adapter, loadChunk) => {
		const state = w.__oxideBoot;
		if (!state) return;
		state.adapter = adapter;
		state.loadChunk = loadChunk;
	};
	w.__oxideBootRun = () => {
		const state = w.__oxideBoot;
		if (!state?.adapter) return;
		const doc = globalThis.document;
		if (!doc) return;
		const opts = {
			doc,
			adapter: state.adapter,
			loadChunk: state.loadChunk
		};
		hydrateIslands(opts);
		loadScripts({ doc });
	};
}
/**
* Register an adapter and its chunk loader, then run the island scan. The
* generated boot entry calls this after its static imports have resolved.
* Safe to call multiple times: the scan is idempotent because each
* `<ox-island>` host carries a `data-ox-hydrated` guard after its first
* hydration attempt.
*
* Runs synchronously if the document is already interactive; otherwise
* waits for `DOMContentLoaded`.
*/
function boot() {
	const w = maybeWindow();
	if (!w) return;
	ensureInstalled(w);
	const run = w.__oxideBootRun;
	if (typeof run !== "function") return;
	const doc = globalThis.document;
	if (!doc) return;
	loadMessages();
	if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", run, { once: true });
	else run();
}
function bootWithRouter(adapter, opts = {}) {
	const doc = opts.doc ?? globalThis.document;
	if (!doc) return () => {};
	setMessageLocales(opts.manifest === void 0 ? void 0 : opts.manifest.i18n_locales ?? []);
	loadMessages();
	const w = globalThis;
	const loadChunk = w.__oxideBoot?.loadChunk ?? (async (c) => {
		try {
			return await import(
				/* @vite-ignore */
				c
);
		} catch (e) {
			console.error?.("[oxide] chunk load failed (fallback); deferred", c, e);
			return;
		}
	});
	setRouteComponentSource(opts.manifest ? {
		manifest: opts.manifest,
		loadChunk
	} : void 0);
	let hydratedRoot = false;
	const root = doc.getElementById?.("root");
	const payload = w.__oxide;
	let payloadData;
	if (root && payload) {
		let data = {};
		let decoded;
		if (typeof payload.data === "string" && payload.data.length > 0) try {
			decoded = decodeOdf(fromBase64(payload.data));
			data = decoded && typeof decoded === "object" && "data" in decoded ? decoded.data : decoded;
		} catch (e) {
			console.warn?.("oxide: window.__oxide.data decode failed; hydrating with {}", e);
		}
		payloadData = data;
		const odfLayouts = decoded && Array.isArray(decoded.layouts) ? decoded.layouts : [];
		const jsLayouts = Array.isArray(payload.layouts) ? payload.layouts : [];
		const layouts = odfLayouts.length > 0 ? odfLayouts : jsLayouts;
		const input = {
			route: payload.route ?? "/",
			url: (w.location?.pathname ?? "/") + (w.location?.search ?? ""),
			params: {},
			data,
			layouts
		};
		try {
			adapter.hydrate(root, input);
			hydratedRoot = true;
		} catch (e) {
			console.error?.("oxide: root hydration failed; SPA router not attached", e);
		}
	}
	try {
		Promise.resolve().then(() => {
			hydrateIslands({
				doc,
				adapter,
				loadChunk
			});
			loadScripts({ doc });
		});
	} catch (e) {
		console.error?.("oxide: island hydration failed", e);
	}
	if (!(payload !== void 0 && payload !== null)) return () => {};
	const routerOnly = !hydratedRoot;
	if (routerOnly && root) return () => {};
	{
		const doc = maybeWindow()?.document;
		if (doc) {
			const scripts = doc.querySelectorAll("script[type=\"module\"]");
			let isEsmDev = false;
			for (let i = 0; i < scripts.length; i++) {
				const src = scripts[i]?.getAttribute?.("src") ?? "";
				if (src.includes("/@boot/") || src.includes("/@prebundle/")) {
					isEsmDev = true;
					break;
				}
			}
			if (isEsmDev) try {
				connectHmr();
			} catch (e) {
				console.warn?.("oxide: HMR websocket failed to open", e);
			}
		}
	}
	let bootGlue;
	if (routerOnly) try {
		bootGlue = attachDeferGlue({ doc });
		if (payloadData !== void 0) bootGlue.wire(payloadData);
	} catch (e) {
		console.warn?.("oxide: boot defer glue failed", e);
		bootGlue = void 0;
	}
	function findLoading(url) {
		const map = opts.loading;
		if (!map) return void 0;
		const urlSegs = (url.split(/[?#]/, 1)[0] || "/").split("/").filter((s) => s.length > 0);
		let bestKey;
		let bestSegs = -1;
		let bestStatic = -1;
		for (const key of Object.keys(map)) {
			const keySegs = ((key || "/").replace(/\/+$/, "") || "/").split("/").filter((s) => s.length > 0);
			if (keySegs.length > urlSegs.length) continue;
			let ok = true;
			let staticCount = 0;
			for (let i = 0; i < keySegs.length; i++) {
				const k = keySegs[i];
				const u = urlSegs[i];
				if (k === void 0 || u === void 0) {
					ok = false;
					break;
				}
				if (isParamSegment(k)) {
					if (u.length === 0) {
						ok = false;
						break;
					}
					continue;
				}
				if (k !== urlSegs[i]) {
					ok = false;
					break;
				}
				staticCount++;
			}
			if (!ok) continue;
			if (keySegs.length > bestSegs || keySegs.length === bestSegs && staticCount > bestStatic) {
				bestKey = key;
				bestSegs = keySegs.length;
				bestStatic = staticCount;
			}
		}
		return bestKey !== void 0 ? map[bestKey] : void 0;
	}
	let loadingDispose;
	let loadingOverlay;
	const onNavigateStart = (url) => {
		if (typeof adapter.renderLoading !== "function") return;
		const Component = findLoading(url);
		if (Component === void 0) return;
		const overlay = doc.createElement("div");
		overlay.setAttribute("data-ox-loading", "");
		(doc.body ?? doc.documentElement)?.appendChild(overlay);
		loadingOverlay = overlay;
		try {
			loadingDispose = adapter.renderLoading(overlay, {
				url,
				route: url,
				params: {},
				data: {}
			}, Component);
		} catch (e) {
			console.warn?.("oxide: renderLoading threw", e);
			overlay.remove();
			loadingOverlay = void 0;
		}
	};
	const onNavigateEnd = () => {
		try {
			loadingDispose?.();
		} catch {}
		loadingDispose = void 0;
		loadingOverlay?.remove();
		loadingOverlay = void 0;
	};
	const onData = async (e) => {
		const detail = e.detail;
		if (!detail || typeof detail !== "object") return;
		let input;
		if ("input" in detail && detail.input) {
			const i = detail.input;
			if (typeof i.route !== "string") return;
			input = {
				route: i.route,
				url: typeof i.url === "string" ? i.url : globalThis.location?.href ?? "/",
				params: i.params ?? {},
				data: i.data
			};
		} else if (typeof detail.odf === "string" && detail.odf.length > 0) {
			let data;
			try {
				const decoded = decodeOdf(fromBase64(detail.odf));
				data = decoded && typeof decoded === "object" && "data" in decoded ? decoded.data : decoded;
			} catch (err) {
				console.warn?.("oxide: ox:data odf decode failed", err);
				return;
			}
			const route = detail.route;
			if (typeof route !== "string") return;
			const loc = globalThis.location;
			const winParams = globalThis.__oxide?.params;
			input = {
				route,
				url: loc?.href ?? "/",
				params: winParams ?? {},
				data
			};
		} else return;
		try {
			adapter.update(input);
		} catch (err) {
			console.warn?.("oxide: ox:data update threw", err);
		}
	};
	doc.addEventListener("ox:data", onData);
	const disposeRouter = createRouter({
		clientBase: opts.clientBase,
		adapter,
		onNavigateStart,
		onNavigateEnd,
		...opts.manifest ? { manifest: opts.manifest } : {}
	}).attach();
	return () => {
		disposeRouter();
		doc.removeEventListener("ox:data", onData);
		bootGlue?.dispose();
		bootGlue = void 0;
	};
}
/**
* Install the boot runtime on `window` without running the island scan.
* Called by the generated entry before `boot()` so `__oxideRegisterAdapter`
* is available when the entry imports the framework adapter. No-op outside
* the browser — safe to call from any module.
*/
function installBootRuntime() {
	const w = maybeWindow();
	if (!w) return;
	ensureInstalled(w);
}
{
	const w = maybeWindow();
	if (w) ensureInstalled(w);
}
export { boot, bootWithRouter, installBootRuntime };

//# sourceMappingURL=client-boot.js.map