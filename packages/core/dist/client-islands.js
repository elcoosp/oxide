import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
// src/client/islands.ts
/** Parse `data-ox-strategy`: `load | idle | visible | media(query)`. */
function parseStrategy(raw) {
	const s = (raw ?? "load").trim();
	if (s.startsWith("media")) return {
		kind: "media",
		query: s.slice(s.indexOf("(") + 1, s.lastIndexOf(")")).trim()
	};
	if (s === "idle" || s === "visible") return { kind: s };
	return { kind: "load" };
}
function b64Bytes(b64) {
	const g = globalThis;
	if (typeof g.atob === "function") {
		const bin = g.atob(b64);
		const bytes = new Uint8Array(bin.length);
		for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
		return bytes;
	}
	if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, "base64"));
	throw new TypeError("islands: no base64 decoder available (atob or Buffer)");
}
/** Decode `data-ox-props`: base64 ODF (§10.3), with a JSON fallback for
* payloads that predate the ODF props encoding (render test fixtures). */
function decodeIslandProps(b64) {
	const bytes = b64Bytes(b64);
	if (bytes.length >= 4 && bytes[0] === 79 && bytes[1] === 68 && bytes[2] === 70 && bytes[3] === 49) return decodeOdf(bytes);
	try {
		return JSON.parse(new TextDecoder().decode(bytes));
	} catch {
		return;
	}
}
/** Hydrate one island now: decode props → load chunk → adapter dispatch.
*
* Two attributes bracket the work so consumers (tests, devtools, a
* readiness check) can distinguish "hydration started" from "hydration
* finished":
*   * `data-ox-hydrated="pending"` — set synchronously on entry, serves as
*     the double-hydration guard.
*   * `data-ox-hydrated="done"` — set after the adapter's `hydrateIsland`
*     returns, i.e. once framework listeners are attached. A browser test
*     that needs to click the island waits for this.
*   * `data-ox-hydrated="error"` — the adapter threw; the island stays
*     inert and the attribute records why.
*/
async function hydrateIsland(el, ctx) {
	const host = el;
	const state = host.getAttribute("data-ox-hydrated");
	if (state === "pending" || state === "done") return void 0;
	host.setAttribute?.("data-ox-hydrated", "pending");
	try {
		const chunk = el.getAttribute("data-ox-chunk") ?? "";
		const propsAttr = el.getAttribute("data-ox-props");
		const props = propsAttr ? decodeIslandProps(propsAttr) : void 0;
		const mod = await ctx.loadChunk(chunk);
		const adapter = ctx.adapter;
		let disposer;
		if (typeof adapter.hydrateIsland === "function") disposer = await Promise.resolve(adapter.hydrateIsland(el, mod, props));
		else if (typeof adapter.hydrate === "function") disposer = adapter.hydrate(el, {
			props,
			mod
		});
		host.setAttribute?.("data-ox-hydrated", "done");
		return disposer;
	} catch (e) {
		host.setAttribute?.("data-ox-hydrated", "error");
		throw e;
	}
}
/**
* Schedule every `<ox-island[data-ox-chunk]>` under `doc` by its strategy
* (§10.3): `load` → now; `idle` → requestIdleCallback (setTimeout fallback);
* `visible` → IntersectionObserver; `media(q)` → matchMedia now/change.
* Returns a disposer that cancels pending schedules.
*/
function hydrateIslands(opts) {
	const doc = opts.doc ?? globalThis.document;
	if (!doc) throw new TypeError("hydrateIslands: no document (pass one in non-browser tests)");
	const loadChunk = opts.loadChunk ?? defaultLoadChunk;
	const ric = opts.requestIdleCallback ?? ((cb) => {
		const g = globalThis;
		return g.requestIdleCallback ? g.requestIdleCallback(cb) : setTimeout(cb, 1);
	});
	const cancelled = { value: false };
	const disposers = [];
	const schedule = (el) => {
		const strategy = parseStrategy(el.getAttribute("data-ox-strategy"));
		const run = () => {
			if (cancelled.value) return;
			hydrateIsland(el, {
				...opts,
				loadChunk
			});
		};
		switch (strategy.kind) {
			case "load":
				run();
				break;
			case "idle":
				ric(() => run());
				break;
			case "visible": {
				const IO = opts.IntersectionObserver ?? globalThis.IntersectionObserver;
				if (!IO) {
					run();
					break;
				}
				const io = new IO((entries) => {
					for (const entry of entries) if (entry.isIntersecting) {
						io.unobserve(entry.target);
						run();
					}
				});
				io.observe(el);
				disposers.push(() => io.unobserve(el));
				break;
			}
			case "media": {
				const mm = opts.matchMedia ?? globalThis.matchMedia;
				if (!mm) break;
				const m = mm(strategy.query);
				if (m.matches) run();
				else if (m.addEventListener) {
					const onChange = () => {
						if (m.matches) {
							m.removeEventListener?.("change", onChange);
							run();
						}
					};
					m.addEventListener("change", onChange);
					disposers.push(() => m.removeEventListener?.("change", onChange));
				}
				break;
			}
		}
	};
	const islands = doc.querySelectorAll("ox-island[data-ox-chunk]");
	for (let i = 0; i < islands.length; i++) {
		const island = islands[i];
		if (island === void 0) continue;
		schedule(island);
	}
	return () => {
		cancelled.value = true;
		for (const d of disposers) d();
	};
}
function defaultLoadChunk(chunk) {
	return import(chunk).catch((e) => {
		console.error?.("[oxide] island chunk fetch failed; island deferred", chunk, e);
	});
}
export { decodeIslandProps, hydrateIsland, hydrateIslands, parseStrategy };

//# sourceMappingURL=client-islands.js.map