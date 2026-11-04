import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
import { layoutChunksFor } from "./client-route-manifest.js";
// src/client/router.ts
/** Minimal DOM surface the router needs (hand-rolled fakes in tests). */
/**
* Narrow a DOM global that may be absent (Node, jsdom-less tests). Callers
* pass the resolved value; a `TypeError` means the caller must pass one
* explicitly. Using a helper (not a cast) keeps the type honest: after this
* returns, TS knows the value is defined, and runtime errors surface at the
* call site, not the first use of the value.
*/
function mustDom(v, name) {
	if (v === void 0) throw new TypeError(`createRouter: no DOM environment (pass ${name} in non-browser tests)`);
	return v;
}
/** Duck-type guard for a click/hover/focus event. The EventTarget contract
*  hands the handler an `unknown`; this proves the fields the router reads
*  exist before the handler narrows. */
function isNavEvent(e) {
	return e !== null && typeof e === "object" && "target" in e;
}
/** Duck-type guard for popstate (`history.state` is the only field read). */
function isPopEvent(e) {
	return e !== null && typeof e === "object" && "state" in e;
}
function createRouter(opts) {
	const g = globalThis;
	const doc = mustDom(opts.doc ?? g.document, "doc");
	const history = mustDom(opts.history ?? g.history, "history");
	const location = mustDom(opts.location ?? g.location, "location");
	const doFetch = opts.fetch ?? ((...a) => globalThis.fetch(...a));
	const scrollTo = opts.scrollTo ?? ((x, y) => g.scrollTo?.(x, y));
	const getScroll = opts.getScroll ?? (() => ({
		x: g.scrollX ?? 0,
		y: g.scrollY ?? 0
	}));
	const IO = opts.IntersectionObserver !== void 0 ? opts.IntersectionObserver : g.IntersectionObserver;
	const budget = opts.prefetchBudget ?? 5;
	if (!doc || !history || !location) throw new TypeError("createRouter: no DOM environment (pass doc/history/location in non-browser tests)");
	const dataCache = /* @__PURE__ */ new Map();
	const DATA_CACHE_MAX = 64;
	function dataCachePut(k, v) {
		dataCache.set(k, v);
		if (dataCache.size > DATA_CACHE_MAX) {
			const oldest = dataCache.keys().next().value;
			if (oldest !== void 0) dataCache.delete(oldest);
		}
	}
	const inFlight = /* @__PURE__ */ new Map();
	let navSeq = 0;
	let disposed = false;
	let io;
	const hrefPath = (href) => {
		try {
			const u = new URL(href, location.href);
			return u.pathname + u.search;
		} catch {
			return href;
		}
	};
	const hrefOf = (el) => el?.getAttribute?.("href") ?? "";
	/** Same-origin, same-target, unmodified navigations are interceptable. */
	function shouldIntercept(a, e) {
		if (e.button !== void 0 && e.button !== 0) return false;
		if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return false;
		if (a.hasAttribute("download")) return false;
		const target = a.getAttribute("target");
		if (target && target !== "_self") return false;
		const href = a.getAttribute("href");
		if (!href) return false;
		if (a.hasAttribute("data-ox-reload")) return false;
		if (a.hasAttribute("data-ox-link")) return true;
		try {
			return new URL(href, location.href).origin === new URL(location.href).origin;
		} catch {
			return false;
		}
	}
	async function fetchRouteData(path, signal) {
		const res = await doFetch(`/__data?route=${encodeURIComponent(path)}`, {
			signal,
			headers: { accept: "application/oxide+odf, application/json" }
		});
		if (!res.ok) throw new Error(`router: /__data responded ${res.status} for ${path}`);
		const bytes = new Uint8Array(await res.arrayBuffer());
		return decodeOdf(bytes);
	}
	function normalizeInput(decoded, path) {
		const d = typeof decoded === "object" && decoded !== null ? decoded : {};
		const input = {
			route: d.route ?? path,
			url: d.url ?? path,
			params: d.params ?? {},
			data: "data" in d ? d.data : d
		};
		if (d.deferred && typeof d.deferred === "object") input.deferred = d.deferred;
		if (typeof d.locale === "string" && d.locale.length > 0) input.locale = d.locale;
		if (Array.isArray(d.layouts)) input.layouts = d.layouts.filter((x) => typeof x === "string");
		return input;
	}
	const routeManifest = opts.manifest;
	const manifestCache = /* @__PURE__ */ new Map();
	function chunksFor(path) {
		if (!routeManifest) return [];
		const hit = manifestCache.get(path);
		if (hit) return hit;
		const segs = path.split("/").filter(Boolean);
		let best;
		let bestStaticCount = -1;
		let matchedRouteId;
		for (const [routeId, entry] of Object.entries(routeManifest.routes)) {
			if (routeId === "__boot__") continue;
			if (!routeId.endsWith("/page") && routeId !== "routes/index/page") continue;
			const m = /^routes\/(.*?)\/page$/.exec(routeId);
			if (!m) continue;
			const routeSegs = m[1] ? m[1].split("/").filter(Boolean) : [];
			const effectiveUrl = segs.length === 0 ? ["index"] : segs;
			if (routeSegs.length !== effectiveUrl.length) continue;
			let staticCount = 0;
			let ok = true;
			for (let i = 0; i < routeSegs.length; i++) {
				const rs = routeSegs[i];
				if (rs === void 0) {
					ok = false;
					break;
				}
				if (/^_|\[/.test(rs)) continue;
				if (rs !== effectiveUrl[i]) {
					ok = false;
					break;
				}
				staticCount++;
			}
			if (!ok) continue;
			if (staticCount > bestStaticCount) {
				bestStaticCount = staticCount;
				best = entry.preload ?? entry.chunks ?? [];
				matchedRouteId = routeId;
			}
		}
		const layoutChunks = matchedRouteId ? layoutChunksFor(matchedRouteId, routeManifest) : [];
		const result = [...best ?? [], ...layoutChunks];
		manifestCache.set(path, result);
		return result;
	}
	/**
	* The base path the client bundle is served from. The boot entry is served
	* as `/client/<boot>.js`, so `import.meta.url` is `<origin>/client/` and the
	* manifest's `entry_chunk` / `preload` values (bare filenames like
	* `routes/index/page.js`) resolve under it. Without this prefix a preload
	* `import('/routes/index/page.js')` 404s silently.
	*
	* The base is derived from `import.meta.url` in a module context; the
	* `clientBase` option remains as an escape hatch for callers that load the
	* router outside a module (it is not injected by the test suite).
	*/
	const CLIENT_BASE = (() => {
		if (opts.clientBase !== void 0) return opts.clientBase;
		try {
			const u = import.meta.url;
			if (typeof u === "string" && u.length > 0) return new URL(".", u).pathname;
		} catch {}
		return "/client/";
	})();
	/** Preload a route's manifest chunks via `import()`; the bundler dedupes. */
	function preloadChunks(path) {
		for (const chunk of chunksFor(path)) (chunk.startsWith("/") ? import(chunk) : import(`${CLIENT_BASE}${chunk}`)).catch(() => {});
	}
	/** Budgeted, cancelable prefetch (§8.4): max `budget` in flight. */
	function prefetch(href) {
		const path = hrefPath(href);
		preloadChunks(path);
		if (disposed || dataCache.has(path) || inFlight.has(path) || inFlight.size >= budget) return;
		const ac = new AbortController();
		inFlight.set(path, ac);
		fetchRouteData(path, ac.signal).then((decoded) => {
			dataCachePut(path, normalizeInput(decoded, path));
		}).catch(() => {}).finally(() => {
			inFlight.delete(path);
		});
	}
	async function goto(href, nav = {}) {
		const path = hrefPath(href);
		const navId = ++navSeq;
		preloadChunks(path);
		let input = dataCache.get(path);
		if (!input) {
			try {
				opts.onNavigateStart?.(path);
			} catch (e) {
				console.warn?.("router: onNavigateStart threw", e);
			}
			try {
				const decoded = await fetchRouteData(path);
				if (navId !== navSeq) return;
				input = normalizeInput(decoded, path);
				dataCachePut(path, input);
			} catch (e) {
				try {
					opts.onNavigateEnd?.({
						route: path,
						url: path,
						params: {},
						data: {}
					});
				} catch {}
				throw e;
			}
		}
		if (navId !== navSeq) {
			try {
				opts.onNavigateEnd?.(input);
			} catch {}
			return;
		}
		const prev = history.state;
		if (prev && prev.__oxide) prev.scroll = getScroll();
		const state = {
			__oxide: 1,
			input,
			scroll: {
				x: 0,
				y: 0
			}
		};
		const commit = () => {
			try {
				opts.onNavigateEnd?.(input);
			} catch (e) {
				console.warn?.("router: onNavigateEnd threw", e);
			}
			history[nav.replace ? "replaceState" : "pushState"](state, "", path);
			opts.adapter.update(input);
		};
		if (doc.startViewTransition) doc.startViewTransition(commit);
		else commit();
	}
	function onClick(e) {
		if (!isNavEvent(e)) return;
		const a = e.target?.closest?.("a");
		if (!a || !shouldIntercept(a, e)) return;
		e.preventDefault?.();
		goto(a.getAttribute("href"));
	}
	function onHover(e) {
		if (!isNavEvent(e)) return;
		const a = e.target?.closest?.("a");
		if (a && shouldIntercept(a, e)) prefetch(a.getAttribute("href"));
	}
	function onPopstate(e) {
		if (!isPopEvent(e)) return;
		const state = e.state;
		if (state && state.__oxide && state.input) {
			opts.adapter.update(state.input);
			if (state.scroll) scrollTo(state.scroll.x, state.scroll.y);
			return;
		}
		const path = location.pathname + location.search;
		const navId = ++navSeq;
		fetchRouteData(path).then((decoded) => {
			if (navId === navSeq) opts.adapter.update(normalizeInput(decoded, path));
		}).catch(() => {});
	}
	const win = doc.defaultView;
	function attach() {
		doc.addEventListener("click", onClick);
		doc.addEventListener("mouseover", onHover);
		doc.addEventListener("focusin", onHover);
		if (win) win.addEventListener("popstate", onPopstate);
		if (IO) {
			io = new IO((entries) => {
				for (const entry of entries) if (entry.isIntersecting) prefetch(hrefOf(entry.target));
			});
			const links = doc.querySelectorAll("a[href]");
			for (let i = 0; i < links.length; i++) {
				const a = links[i];
				if (shouldIntercept(a, {})) io.observe(a);
			}
		}
		return dispose;
	}
	function dispose() {
		disposed = true;
		doc.removeEventListener("click", onClick);
		doc.removeEventListener("mouseover", onHover);
		doc.removeEventListener("focusin", onHover);
		if (win) win.removeEventListener("popstate", onPopstate);
		io?.disconnect();
		io = void 0;
		for (const ac of inFlight.values()) ac.abort();
		inFlight.clear();
	}
	return {
		attach,
		dispose,
		goto,
		prefetch,
		current: () => {
			return history.state?.input ?? dataCache.get(location.pathname + location.search);
		}
	};
}
export { createRouter };

//# sourceMappingURL=client-router.js.map