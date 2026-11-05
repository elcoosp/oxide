import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
// src/client/hmr.ts
/** Default WebSocket URL for the current page. */
function defaultUrl() {
	const loc = globalThis.location;
	if (!loc) return "";
	return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/_oxide/hmr`;
}
/**
* §1.3: does a route PATTERN (`/blog/[slug]`, `/`, `/greeting`) match the
* concrete `pathname` the browser is on? Segment-wise: a static segment must
* equal; `[param]` / `:param` / `*` match one non-empty segment. Used to drop
* `route-data-update` messages for routes the viewer is NOT on — the boot
* window (and an edit to another page's loader) publishes fresh loader data
* for foreign routes, and blindly feeding it to `adapter.update` rendered
* THAT route in place: a phantom navigation with no URL change that replaced
* the visible page with a different one.
*/
function routePatternMatchesPath(pattern, pathname) {
	const pSegs = pattern.split("/").filter((s) => s.length > 0);
	const pathSegs = pathname.split("?")[0]?.split("#")[0]?.split("/").filter((s) => s.length > 0) ?? [];
	if (pSegs.length !== pathSegs.length) return false;
	for (let i = 0; i < pSegs.length; i++) {
		const p = pSegs[i] ?? "";
		const seg = pathSegs[i] ?? "";
		if (p.startsWith("[") || p.startsWith(":") || p === "*" || p.startsWith("*")) continue;
		if (p !== seg) return false;
	}
	return true;
}
/** Swap the <link rel=stylesheet> whose href matches `path` (query-stripped). */
function swapCss(doc, path) {
	const bare = path.split("?").at(0) ?? "";
	const links = doc.querySelectorAll("link[rel=\"stylesheet\"]");
	for (let i = 0; i < links.length; i++) {
		const el = links[i];
		if (el === void 0) continue;
		const linkEl = el;
		const href = (linkEl.getAttribute("href") ?? "").split("?").at(0) ?? "";
		if (href === bare || href.endsWith(bare)) {
			linkEl.setAttribute("href", `${bare}?t=${Date.now()}`);
			return true;
		}
	}
	return false;
}
/** Render (or replace) the diagnostic overlay. */
function showOverlay(diag, parent) {
	const doc = globalThis.document;
	if (!doc) return void 0;
	const root = parent ?? doc.body;
	const id = "ox-hmr-overlay";
	let el = doc.getElementById(id);
	if (!el) {
		el = doc.createElement("div");
		el.id = id;
		el.style.cssText = "position:fixed;inset:auto 16px 16px 16px;z-index:2147483647;background:#1a0f0f;color:#ffd7d7;border:1px solid #a33;border-radius:8px;font:13px/1.5 ui-monospace,monospace;padding:14px 16px;max-height:40vh;overflow:auto;box-shadow:0 8px 24px rgba(0,0,0,.5)";
		root.appendChild(el);
	}
	const capturedEl = el;
	const code = diag.code ?? "ERROR";
	const msg = diag.message ?? "";
	el.innerHTML = "";
	const head = doc.createElement("div");
	head.style.cssText = "display:flex;justify-content:space-between;align-items:center;margin-bottom:8px";
	const title = doc.createElement("strong");
	title.textContent = code;
	const close = doc.createElement("button");
	close.textContent = "×";
	close.style.cssText = "background:none;border:0;color:inherit;font-size:18px;cursor:pointer";
	close.addEventListener("click", () => capturedEl.remove());
	head.appendChild(title);
	head.appendChild(close);
	const body = doc.createElement("pre");
	body.style.cssText = "white-space:pre-wrap;word-break:break-word;margin:0";
	body.textContent = msg;
	el.appendChild(head);
	el.appendChild(body);
	if (diag.fix) {
		const fix = doc.createElement("div");
		fix.style.cssText = "margin-top:8px;color:#9fdc9f";
		fix.textContent = `fix: ${diag.fix}`;
		el.appendChild(fix);
	}
	return el;
}
/**
* §13.3 `route-data-update`: a page loader changed in dev. The dev server
* publishes the route as a FILE path (not a URL) alongside the freshly
* rendered loader payload as `odf_b64`, so the refresh prefers decoding that
* payload inline and only falls back to re-fetching `/__data` for the CURRENT
* location when it is absent, then dispatches
* `ox:data` on the document with the normalized ClientInput — the same
* event `bootWithRouter` consumes to call `adapter.update` (the same data
* path a navigation uses, §8.4). All failures are silent: an HMR data
* refresh is best-effort and must never break the socket loop.
*/
async function refreshRouteData(route, odfB64) {
	const g = globalThis;
	const locPath = g.location?.pathname;
	if (locPath && !routePatternMatchesPath(route, locPath)) return;
	const path = (locPath ?? "/") + (g.location?.search ?? "");
	let decoded;
	if (odfB64.length > 0) try {
		const bin = atob(odfB64);
		const bytes = new Uint8Array(bin.length);
		for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
		decoded = decodeOdf(bytes);
	} catch {
		return;
	}
	else {
		if (typeof g.fetch !== "function") return;
		try {
			const res = await g.fetch(`/__data?route=${encodeURIComponent(path)}`, { headers: { accept: "application/oxide+odf, application/json" } });
			if (!res.ok) return;
			decoded = decodeOdf(new Uint8Array(await res.arrayBuffer()));
		} catch {
			return;
		}
	}
	const d = typeof decoded === "object" && decoded !== null ? decoded : {};
	const input = {
		route: d.route ?? path,
		url: d.url ?? path,
		params: d.params ?? {},
		data: "data" in d ? d.data : decoded
	};
	try {
		const detail = {
			route: path,
			input
		};
		const ev = typeof g.CustomEvent === "function" ? new g.CustomEvent("ox:data", { detail }) : {
			type: "ox:data",
			detail
		};
		g.document?.dispatchEvent?.(ev);
	} catch {}
}
/**
* Remove the diagnostic overlay if present. Called when a subsequent
* `module-update` (or successful re-import) proves the earlier diagnostic is
* stale — the user fixed the file and the next edit applied cleanly.
*/
function clearOverlay(parent) {
	const doc = globalThis.document;
	if (!doc) return;
	const el = (parent ?? doc.body)?.querySelector?.("#ox-hmr-overlay") ?? doc.getElementById("ox-hmr-overlay");
	if (el) el.remove();
}
/** Apply one HMR message. Returns true when handled. */
function applyHmrMessage(msg, opts = {}) {
	const doc = globalThis.document;
	if (!doc) return false;
	switch (msg.type) {
		case "css-update": {
			const path = String(msg.path ?? "");
			swapCss(doc, path);
			opts.onCssUpdate?.(path);
			return true;
		}
		case "route-data-update": {
			const route = String(msg.route ?? "");
			const odf = String(msg.odf_b64 ?? "");
			opts.onRouteData?.(route, odf);
			refreshRouteData(route, odf);
			return true;
		}
		case "module-update": {
			const path = String(msg.path ?? "");
			const url = typeof msg.url === "string" ? msg.url : "";
			clearOverlay(opts.overlayParent);
			const hmr = globalThis.__oxide_hmr__;
			if (url && typeof hmr?.reloadModule === "function") {
				hmr.reloadModule(url);
				return true;
			}
			if (opts.onModuleUpdate) {
				opts.onModuleUpdate(path);
				return true;
			}
			(opts.reload ?? (() => globalThis.location?.reload()))?.();
			return true;
		}
		case "batch-module-update": {
			const items = Array.isArray(msg.items) ? msg.items : [];
			clearOverlay(opts.overlayParent);
			const hmr = globalThis.__oxide_hmr__;
			if (typeof hmr?.reloadModule === "function") {
				for (const item of items) {
					const url = typeof item.url === "string" ? item.url : "";
					if (url) hmr.reloadModule(url);
				}
				return true;
			}
			(opts.reload ?? (() => globalThis.location?.reload()))?.();
			return true;
		}
		case "server-module-swap": return true;
		case "full-reload":
			(opts.reload ?? (() => globalThis.location?.reload()))?.();
			return true;
		case "error":
			showOverlay(msg.diagnostic ?? {}, opts.overlayParent);
			return true;
		case "connected": return true;
		default: return false;
	}
}
/** Open the HMR WebSocket; returns a disposer. No-op outside a browser. */
function connectHmr(opts = {}) {
	const g = globalThis;
	if (typeof g.WebSocket !== "function" || !g.document) return () => {};
	const url = opts.url ?? defaultUrl();
	if (!url) return () => {};
	let closed = false;
	let socket;
	const open = () => {
		if (closed) return;
		socket = new g.WebSocket(url);
		socket.onmessage = (ev) => {
			try {
				applyHmrMessage(JSON.parse(String(ev.data)), opts);
			} catch {}
		};
		socket.onclose = () => {
			socket = void 0;
			if (!closed) setTimeout(open, opts.reconnectMs ?? 1e3);
		};
		socket.onerror = () => {
			socket?.close();
		};
	};
	open();
	return () => {
		closed = true;
		socket?.close();
	};
}
// src/client/hot.ts
const modules = /* @__PURE__ */ new Map();
let reloadSeq = 0;
const lastReloadAt = /* @__PURE__ */ new Map();
const trailingTimers = /* @__PURE__ */ new Map();
const COALESCE_MS = 100;
const MAX_TRACKED_MODULES = 500;
const MAX_TRACKED_URLS = 500;
function install() {
	const hasWindow = typeof window !== "undefined";
	const hasWorkerScope = typeof self !== "undefined" && typeof self.importScripts === "function";
	if (!hasWindow && !hasWorkerScope) return;
	const g = hasWindow ? window : self;
	if (g.__oxide_hmr__) return;
	g.__oxide_hmr__ = {
		createHotContext(url) {
			let mod = modules.get(url);
			if (!mod) {
				mod = {
					url,
					data: {},
					acceptCallbacks: [],
					disposeCallbacks: [],
					invalidated: false
				};
				modules.set(url, mod);
				if (modules.size > MAX_TRACKED_MODULES) {
					const oldest = modules.keys().next().value;
					if (oldest !== void 0) modules.delete(oldest);
				}
			}
			const m = mod;
			return {
				get url() {
					return url;
				},
				data: m.data,
				accept(cb) {
					if (cb) m.acceptCallbacks.push(cb);
				},
				dispose(cb) {
					if (cb) m.disposeCallbacks.push(cb);
				},
				invalidate() {
					m.invalidated = true;
				}
			};
		},
		async reloadModule(url) {
			const __now = Date.now();
			if (__now - (lastReloadAt.get(url) ?? 0) < COALESCE_MS) {
				if (!trailingTimers.has(url)) {
					const t = setTimeout(() => {
						trailingTimers.delete(url);
						globalThis.__oxide_hmr__?.reloadModule?.(url);
					}, 120);
					trailingTimers.set(url, t);
				}
				return;
			}
			lastReloadAt.delete(url);
			lastReloadAt.set(url, __now);
			if (lastReloadAt.size > MAX_TRACKED_URLS) {
				const oldest = lastReloadAt.keys().next().value;
				if (oldest !== void 0) lastReloadAt.delete(oldest);
			}
			const mod = modules.get(url);
			if (mod) {
				for (const cb of mod.disposeCallbacks) try {
					cb();
				} catch {}
				mod.disposeCallbacks.length = 0;
				mod.acceptCallbacks.length = 0;
				mod.invalidated = false;
			}
			const freshUrl = `${url}${url.includes("?") ? "&" : "?"}t=${Date.now()}-${++reloadSeq}`;
			let fresh;
			try {
				fresh = await import(
					/* @vite-ignore */
					freshUrl
);
			} catch (err) {
				console.error("[oxide] module reload failed:", url, err);
				showOverlay({
					code: "HMR-IMPORT",
					message: err instanceof Error ? err.message : String(err),
					fix: "fix the file and save again — no reload needed"
				});
				return;
			}
			const freshMod = modules.get(url);
			if (freshMod) {
				for (const cb of freshMod.acceptCallbacks) try {
					cb(fresh);
				} catch {}
				freshMod.acceptCallbacks.length = 0;
			}
		},
		__debug: {
			get moduleCount() {
				return modules.size;
			},
			get reloadCount() {
				return lastReloadAt.size;
			},
			get MAX_MODULES() {
				return MAX_TRACKED_MODULES;
			},
			get MAX_URLS() {
				return MAX_TRACKED_URLS;
			}
		}
	};
	let pendingRuntimeLoad;
	if (hasWindow && typeof g.__oxide_refresh === "undefined") g.__oxide_refresh = (_mod) => {
		const perform = () => {
			const rt = g.$RefreshRuntime$;
			if (rt && typeof rt.performReactRefresh === "function") try {
				rt.performReactRefresh();
				return;
			} catch (e) {
				console.error("[hot] performReactRefresh() threw:", e);
				return;
			}
			if (typeof location !== "undefined") location.reload();
		};
		if (g.$RefreshRuntime$) {
			perform();
			return;
		}
		if (pendingRuntimeLoad) {
			pendingRuntimeLoad.then(perform).catch(() => {
				if (typeof location !== "undefined") location.reload();
			});
			return;
		}
		perform();
	};
	const bufferedSigs = [];
	if (hasWindow && typeof g.__oxide_refresh_sig === "undefined") g.__oxide_refresh_sig = () => {
		const rt = g.$RefreshRuntime$;
		if (rt && typeof rt.createSignatureFunctionForTransform === "function") try {
			return rt.createSignatureFunctionForTransform();
		} catch (e) {
			console.error("[hot] createSignatureFunctionForTransform() threw:", e);
		}
		const buffered = { calls: [] };
		bufferedSigs.push(buffered);
		return (...args) => {
			if (buffered.real) return buffered.real(...args);
			buffered.calls.push(args);
			if (args.length >= 2 && typeof args[1] === "string") return args[0];
		};
	};
	const pendingRegistrations = [];
	if (hasWindow && typeof g.__oxide_refresh_reg === "undefined") g.__oxide_refresh_reg = (t, id) => {
		const rt = g.$RefreshRuntime$;
		if (rt && typeof rt.register === "function") try {
			rt.register(t, id);
		} catch {}
		else pendingRegistrations.push({
			t,
			id
		});
	};
	if (hasWindow && typeof g.$RefreshRuntime$ === "undefined") {
		const nav = typeof window !== "undefined" ? window : globalThis;
		const scheme = nav.location?.protocol ?? "";
		let isEsmDev = false;
		if (scheme === "http:" || scheme === "https:") {
			const scripts = nav.document?.querySelectorAll("script[type=\"module\"]");
			if (scripts) for (let i = 0; i < scripts.length; i++) {
				const src = scripts[i]?.getAttribute?.("src") ?? "";
				if (src.includes("/@boot/") || src.includes("/@prebundle/")) {
					isEsmDev = true;
					break;
				}
			}
		}
		if (isEsmDev) pendingRuntimeLoad = import(
			/* @vite-ignore */
			"/@prebundle/react-refresh-runtime.js"
).then((mod) => {
			const rt = mod.default ?? mod;
			if (typeof rt.injectIntoGlobalHook === "function") try {
				rt.injectIntoGlobalHook(window);
			} catch (e) {}
			g.$RefreshRuntime$ = rt;
			if (typeof rt.register === "function") for (const { t, id } of pendingRegistrations) try {
				rt.register(t, id);
			} catch {}
			pendingRegistrations.length = 0;
			if (bufferedSigs.length > 0 && typeof rt.createSignatureFunctionForTransform === "function") {
				for (const b of bufferedSigs) {
					if (b.real) continue;
					try {
						b.real = rt.createSignatureFunctionForTransform();
						for (const args of b.calls) try {
							b.real(...args);
						} catch {}
						b.calls.length = 0;
					} catch {}
				}
				bufferedSigs.length = 0;
			}
		}).catch(() => {});
	}
}
install();
(function connectWorkerHmr() {
	const hasWindow = typeof window !== "undefined";
	const hasWorkerScope = typeof self !== "undefined" && typeof self.importScripts === "function";
	if (hasWindow || !hasWorkerScope) return;
	const g = self;
	if (typeof g.WebSocket !== "function" || !g.location) return;
	const url = `${g.location.protocol === "https:" ? "wss" : "ws"}://${g.location.host}/_oxide/hmr`;
	let socket;
	const open = () => {
		socket = new g.WebSocket(url);
		socket.onmessage = (ev) => {
			try {
				const raw = typeof ev.data === "string" ? ev.data : String(ev.data);
				const msg = JSON.parse(raw);
				const hmr = g.__oxide_hmr__;
				if (typeof hmr?.reloadModule !== "function") return;
				if (msg.type === "module-update" && typeof msg.url === "string" && msg.url) hmr.reloadModule(msg.url);
				else if (msg.type === "batch-module-update" && Array.isArray(msg.items)) {
					for (const it of msg.items) if (typeof it.url === "string" && it.url) hmr.reloadModule(it.url);
				}
			} catch {}
		};
		socket.onclose = () => {
			socket = void 0;
			setTimeout(open, 1e3);
		};
		socket.onerror = () => {
			socket?.close();
		};
	};
	open();
})();
export { connectHmr as t };

//# sourceMappingURL=client-hot.js.map