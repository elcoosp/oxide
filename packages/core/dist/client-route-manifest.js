// src/client/route-manifest.ts
function oxideGlobal() {
	return globalThis;
}
/**
* Register (or clear, with `undefined`) the manifest + chunk loader the
* adapters resolve page components from. Called by `bootWithRouter` when the
* boot entry hands it a manifest; idempotent per boot.
*
* State lives on `globalThis`, not a module-local binding, because the client
* bundler can emit this module more than once in one page: the boot entry and
* a framework adapter's chunk reach `route-manifest.ts` through different
* relative specifiers, so Rolldown treats them as two module instances. A
* module-local binding would be written by the boot copy and read (still
* `undefined`) by the adapter copy, leaving every navigation on the IR
* fallback — the client-side analogue of the SSR two-React bug. `globalThis`
* is per-realm, so every bundle copy shares one slot. In Node/SSR there is a
* single instance, so the behaviour is unchanged.
*/
function setRouteComponentSource(next) {
	oxideGlobal().__oxide_fw_source__ = next;
}
/** The currently registered source, or `undefined` (IR fallback territory). */
function getRouteComponentSource() {
	return oxideGlobal().__oxide_fw_source__;
}
/**
* True when a path segment is a dynamic-route parameter token. Covers every
* spelling the build emits: `[name]` and `[...rest]` (the scanner keeps the
* bracket dir token verbatim), `_name_` (the manifest's normalized form), and
* `:name` (hand-written route tables). Shared by the {@link routeIdFor}
* matcher and boot.ts's loading-map matcher (§8.4).
*/
function isParamSegment(seg) {
	if (seg.startsWith(":")) return seg.length > 1;
	if (seg.startsWith("[") && seg.endsWith("]")) return seg.length > 2;
	return seg.length > 2 && seg.startsWith("_") && seg.endsWith("_");
}
/** Split a URL path or manifest-key body into non-empty segments. */
function segmentsOf(path) {
	return path.split("/").filter((s) => s.length > 0);
}
/**
* Normalize a single dynamic-segment token to the manifest's spelling.
*
* The scanner keeps `[name]` / `[...name]` verbatim in route IDs
* (`routes/blog/[slug]`), but the client manifest normalizes them to
* `_name_` (`routes/blog/_slug_/page`) — see the basename normalization in
* `crates/jshost/src/lib.rs`. SSR's ODF payload and `window.__oxide` still
* carry the source spelling, so {@link routeIdFor} receives the label form
* and must normalize before matching manifest keys. Otherwise a label like
* `routes/docs/[page]` never resolves against the manifest's
* `routes/docs/_page_/page`, `tryRealRender` logs "route not matched in
* manifest", and client navigation falls back to the IR renderer — the
* symptom is a 404 page on CSR while a hard reload (SSR path) works.
*
* Idempotent on already-normalized / non-dynamic segments.
*/
function normalizeDynamicSegment(seg) {
	if (seg.startsWith("[") && seg.endsWith("]") && seg.length > 2) {
		const inner = seg.slice(1, -1);
		const name = inner.startsWith("...") ? inner.slice(3) : inner;
		if (name.length > 0) return `_${name}_`;
	}
	return seg;
}
/**
* Does a manifest key's segment list match a URL's segment list?
*
* Matching rules (§8.4, the same contract the CLI's loading-map keys use):
*   - a literal key segment equals the URL segment;
*   - a param key segment (`[name]`, `[...name]`, `_name_`, `:name`) matches
*     exactly ONE non-empty URL segment;
*   - `routes/index/page` is the manifest key for the root URL `/`
*     (normalized to the literal segment `index` before comparison, so it
*     only matches the root, not `/index`);
*   - the key may be a PREFIX of the URL (a layout-level boundary covers the
*     routes below it) — {@link routeIdFor} only uses full matches, the
*     loading-map matcher in `boot.ts` uses prefix matches.
*
* Returns the match quality (static-segment count) when the key matches
* `urlSegs` in full, or `undefined` when it doesn't.
*/
function matchKeySegments(keySegs, urlSegs) {
	if (keySegs.length !== urlSegs.length) return void 0;
	let staticCount = 0;
	for (let i = 0; i < keySegs.length; i++) {
		const k = keySegs[i];
		const u = urlSegs[i];
		if (k === void 0 || u === void 0) return void 0;
		if (isParamSegment(k)) {
			if (u.length === 0) return void 0;
			continue;
		}
		if (k !== u) return void 0;
		staticCount++;
	}
	return staticCount;
}
/**
* Resolve the manifest route id for a route reference. Accepts every shape
* the runtime hands adapters:
*   - a full manifest key: `routes/blog/[slug]/page`;
*   - a route label (what `/__data` and `window.__oxide` carry):
*     `routes/blog/[slug]` — the scanner id without the `/page` suffix;
*   - a URL path: `/blog/hello`, `/` (segment-matched against the keys).
*
* Returns the exact manifest key, or `undefined` when the manifest has no
* route for it (callers fall back to the IR renderer). Ties between a param
* and a literal segment at the same depth resolve to the literal (higher
* static-segment count), mirroring the router's own matcher.
*/
function routeIdFor(routeRef, manifest) {
	if (!routeRef || typeof routeRef !== "string") return void 0;
	const ref = routeRef.split(/[?#]/, 1)[0] ?? routeRef;
	const routes = manifest?.routes ?? {};
	const isPageKey = (k) => k === "routes/index/page" || /^routes\/.+\/page$/.test(k);
	if (routes[ref] !== void 0 && isPageKey(ref)) return ref;
	const normalizedRef = segmentsOf(ref).map(normalizeDynamicSegment).join("/");
	if (!ref.endsWith("/page") && normalizedRef.length > 0) {
		const canonical = `${normalizedRef}/page`;
		if (routes[canonical] !== void 0 && isPageKey(canonical)) return canonical;
		if (normalizedRef !== ref) {
			const literal = `${ref}/page`;
			if (routes[literal] !== void 0 && isPageKey(literal)) return literal;
		}
	}
	let urlSegs = segmentsOf(ref.replace(/^\/+/, ""));
	if (urlSegs.length > 0 && urlSegs[0] === "routes") urlSegs = urlSegs.slice(1).map(normalizeDynamicSegment);
	if (ref === "/" || ref === "" || ref === "routes/index" || ref === "index") urlSegs = [];
	let best;
	let bestStatic = -1;
	for (const key of Object.keys(routes)) {
		if (key === "__boot__") continue;
		const m = /^routes\/(.*?)\/page$/.exec(key);
		if (!m) continue;
		const keySegs = m[1] ? segmentsOf(m[1]) : [];
		const staticCount = matchKeySegments(keySegs.length === 1 && keySegs[0] === "index" ? [] : keySegs, urlSegs);
		if (staticCount === void 0) continue;
		if (staticCount > bestStatic) {
			bestStatic = staticCount;
			best = key;
		}
	}
	return best;
}
/**
* §8.2 / O1: the layout chunks a route needs, given a matched manifest key.
*
* Each page's manifest entry carries `layout_chain` — the layout route ids
* (e.g. `routes/layout`), which the bundler emits directly as RouteBundles
* with no page suffix. This walks the chain and returns the layout chunk
* names (entry + preload) so a preload walk can warm them in parallel with
* the page.
*
* Pure and dependency-free: the caller has already resolved the page's
* manifest key. Returns an empty array when the route has no entry, no
* `layout_chain`, or a chain referencing ids the manifest does not carry —
* a missing layout chunk is simply not preloaded, never an error.
*/
function layoutChunksFor(routeId, manifest) {
	const entry = manifest?.routes?.[routeId];
	if (!entry?.layout_chain || entry.layout_chain.length === 0) return [];
	const out = [];
	for (const layoutId of entry.layout_chain) {
		const layout = manifest.routes[layoutId];
		if (!layout) continue;
		const layoutRef = layout.component ?? layout.entry_chunk;
		if (layoutRef) out.push(layoutRef);
		if (layout.preload) out.push(...layout.preload);
	}
	return out;
}
/**
* Extract the page component export from a dynamically imported route chunk
* module: `default` first, then the conventional named exports, then the
* module itself when it is a plain component function. Returns `undefined`
* when nothing component-shaped is found (callers fall back to IR).
*/
function pageComponentExport(mod) {
	if (mod === void 0 || mod === null) return void 0;
	const m = mod;
	const candidates = [
		m.default,
		m.Page,
		m.page,
		m.Component,
		m.component
	];
	for (const c of candidates) if (c !== void 0 && c !== null) return c;
	if (typeof mod === "function") return mod;
}
/** Turn a manifest chunk path into a root-relative URL the importer can fetch. */
function chunkUrl(chunk) {
	if (/^(https?:)?\/\//.test(chunk) || chunk.startsWith("/")) return chunk;
	return `/client/${chunk}`;
}
/**
* Manifest-driven `componentFor(routeId)` (§8.4): resolve the route's
* manifest entry, dynamically import its client entry chunk through the
* registered {@link RouteComponentSource.loadChunk}, and return the page
* component export. Returns `undefined` (never throws) when no source is
* registered, the route isn't in the manifest, the chunk fails to load, or
* the module carries no component export — every caller must treat that as
* "fall back to the IR renderer".
*/
async function componentFor(routeRef) {
	const src = getRouteComponentSource();
	if (!src) return void 0;
	const key = (src.manifest.routes[routeRef] !== void 0 ? routeRef : void 0) ?? routeIdFor(routeRef, src.manifest);
	if (key === void 0) return void 0;
	const entry = src.manifest.routes[key];
	if (!entry) return void 0;
	const chunk = entry.component ?? entry.entry_chunk ?? entry.preload?.[0] ?? entry.chunks?.[0];
	if (!chunk) return void 0;
	try {
		const mod = await src.loadChunk(chunkUrl(chunk));
		const result = pageComponentExport(mod);
		if (result === void 0) {
			const modKeys = mod && typeof mod === "object" ? Object.keys(mod) : typeof mod;
			console.warn?.("[oxide-diag] componentFor: chunk loaded but no component export (chunk=" + JSON.stringify(chunk) + " url=" + JSON.stringify(chunkUrl(chunk)) + " module-keys=" + JSON.stringify(modKeys) + ")");
		}
		return result;
	} catch (e) {
		console.warn?.("[oxide-diag] componentFor: loadChunk threw (chunk=" + JSON.stringify(chunk) + " url=" + JSON.stringify(chunkUrl(chunk)) + ")", e);
		return;
	}
}
export { componentFor, getRouteComponentSource, isParamSegment, layoutChunksFor, pageComponentExport, routeIdFor, setRouteComponentSource };

//# sourceMappingURL=client-route-manifest.js.map