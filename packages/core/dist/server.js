import { a as decodeOdf } from "./odf-decode-D1PLHGuy.js";
import { bytesToBase64 } from "./bytes.js";
import { BOUNDARY_CONTENT_ATTR, BOUNDARY_FALLBACK_ATTR, beginBoundaryCapture, boundaryContent, boundaryFallback, deferredIdOf, drainBoundaries, endBoundaryCapture, registerBoundary } from "./server-boundary.js";
import { encodeOdf } from "./odf-encode.js";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join } from "node:path";
// src/server/content.ts
const cache$1 = /* @__PURE__ */ new Map();
function projectRoot() {
	return process.env.OXIDE_ROOT ?? process.cwd();
}
/**
* Read every entry in a collection. Throws when the collection was not
* built (a typo in the name, or a project that has never run `oxide
* build`) — the error message names the expected file so a fix is one
* step away.
*/
async function getCollection(name) {
	if (!name || name.length === 0) throw new Error("getCollection: name is required");
	const path = join(projectRoot(), "dist", "content", `${name}.entries.json`);
	let raw;
	try {
		raw = await readFile(path, "utf8");
	} catch {
		throw new Error(`getCollection(${JSON.stringify(name)}): no entries file at ${path}. Is "${name}" in content.collections in oxide.config.ts, and has \`oxide build\` run since it was added?`);
	}
	const hash = createHash("sha256").update(raw).digest("hex");
	const cached = cache$1.get(path);
	if (cached && cached.hash === hash) return cached.entries;
	const doc = JSON.parse(raw);
	const entries = Array.isArray(doc.entries) ? doc.entries : [];
	cache$1.set(path, {
		hash,
		entries
	});
	return entries;
}
/**
* Read one entry by slug. Returns `undefined` when the slug is absent
* from the collection (a miss in a loader is often a 404, not an error).
* Throws only when the *collection* itself is missing — the same
* contract as `getCollection`.
*/
async function getEntry(collection, slug) {
	return (await getCollection(collection)).find((e) => e.slug === slug);
}
/**
* Test-only: clear the mtime cache. The public surface is
* `getCollection` / `getEntry`; a test that reads the same file after
* rewriting it needs a fresh stat.
*/
function __clearContentCache() {
	cache$1.clear();
}
// src/server/index.ts
/** Tag key on load fns (`fn.$$oxide === 'load'`). */
const OXIDE_TAG = "$$oxide";
/**
* Tag a load function. Calling the returned fn runs the load with the event's
* `defer` wired to a per-request id registry: promises registered via
* `event.defer(p)` get `{__deferred: id}` placeholders immediately, and any
* standalone `defer(p)` markers found in the returned data are converted on
* the way out (Value::Deferred-compatible shape, §9.5). The Rust render
* pipeline consumes the registry via `event.__oxideDeferredRegistry`.
*/
function defineLoad(fn) {
	const wrapped = async (event) => {
		const holder = event;
		const registry = holder.__oxideDeferredRegistry instanceof Map ? holder.__oxideDeferredRegistry : /* @__PURE__ */ new Map();
		holder.__oxideDeferredRegistry = registry;
		let nextId = 1;
		const allocate = () => nextId++;
		return convertDefers(await fn({
			...event,
			defer: (promise) => {
				const id = allocate();
				registry.set(id, promise);
				return { __deferred: id };
			}
		}), registry, allocate);
	};
	return Object.assign(wrapped, { $$oxide: "load" });
}
/** A standalone deferred marker (what the exported [defer] returns). */
function isDeferMarker(v) {
	return typeof v === "object" && v !== null && v.__oxide_defer === true && typeof v.promise === "object";
}
/**
* Replace standalone [defer] markers in a load result with
* `{__deferred: id}` placeholders and register their promises. Cycle-safe
* (WeakSet) and depth-capped (§17.1 parity with the ODF decoder).
*/
function convertDefers(value, registry, allocate, depth = 1, seen = /* @__PURE__ */ new WeakSet()) {
	if (depth > 512 || value === null || typeof value !== "object") return value;
	if (isDeferMarker(value)) {
		const id = allocate();
		registry.set(id, value.promise);
		return { __deferred: id };
	}
	if (seen.has(value)) return value;
	seen.add(value);
	if (Array.isArray(value)) {
		for (let i = 0; i < value.length; i++) value[i] = convertDefers(value[i], registry, allocate, depth + 1, seen);
		return value;
	}
	if (value instanceof Map) {
		for (const [k, v] of value) value.set(k, convertDefers(v, registry, allocate, depth + 1, seen));
		return value;
	}
	if (value instanceof Set) return new Set([...value].map((v) => convertDefers(v, registry, allocate, depth + 1, seen)));
	const obj = value;
	for (const k of Object.keys(obj)) {
		if (k === "__proto__" || k === "constructor") continue;
		obj[k] = convertDefers(obj[k], registry, allocate, depth + 1, seen);
	}
	return obj;
}
function defer(promise) {
	return {
		__oxide_defer: true,
		promise
	};
}
/** Thrown by [error]; carries the HTTP status for the response pipeline. */
var OxideHttpError = class extends Error {
	status;
	constructor(status, message) {
		super(message ?? httpStatusText(status));
		this.name = "OxideHttpError";
		this.status = status;
	}
};
/** Thrown by [redirect]; the pipeline converts it to a redirect response. */
var OxideRedirect = class extends Error {
	location;
	status;
	constructor(location, status = 303) {
		super(`redirecting to ${location}`);
		this.name = "OxideRedirect";
		this.location = location;
		this.status = status;
	}
};
const STATUS_TEXT = {
	400: "Bad Request",
	401: "Unauthorized",
	403: "Forbidden",
	404: "Not Found",
	405: "Method Not Allowed",
	409: "Conflict",
	410: "Gone",
	418: "I'm a teapot",
	422: "Unprocessable Entity",
	429: "Too Many Requests",
	500: "Internal Server Error",
	502: "Bad Gateway",
	503: "Service Unavailable",
	504: "Gateway Timeout"
};
function httpStatusText(status) {
	return STATUS_TEXT[status] ?? `HTTP ${status}`;
}
/** Abort a load with an HTTP error (§9.2). Throws [OxideHttpError]. */
function error(status, message) {
	throw new OxideHttpError(status, message);
}
/** Redirect from a load or action. Default 303 (POST→GET safe). Throws. */
function redirect(location, status = 303) {
	throw new OxideRedirect(location, status);
}
/** Tag an action definition (form-first, §9.3). */
function defineAction(def) {
	return {
		$$oxide: "action",
		input: def.input,
		run: def.run
	};
}
/** Tag a meta function: load data → neutral head model (§16.3). */
function defineMeta(fn) {
	return Object.assign(fn, { $$oxide: "meta" });
}
/** Tag a WebSocket route's handlers (§9.6 — Rust owns framing/backpressure). */
function defineSocket(handlers) {
	return {
		...handlers,
		$$oxide: "socket"
	};
}
/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
function cacheKey(args, opts) {
	const normalized = {
		ttl: opts?.ttl ?? null,
		swr: opts?.swr ?? null,
		tags: [...opts?.tags ?? []].sort()
	};
	return JSON.stringify({
		args,
		opts: normalized
	});
}
/** True when a Rust host is reachable through the op bridge. */
function hostOpAvailable() {
	return typeof globalThis.__oxide_op_request === "function";
}
const cacheStore = /* @__PURE__ */ new Map();
const cachePending = /* @__PURE__ */ new Map();
/** P-10: bound the in-process fallback cache. Keys derive from caller args,
*  which an attacker controls, so without a cap a long-lived worker's Map
*  grows without limit. Expired entries are swept periodically and a hard
*  entry cap evicts oldest-first. */
const CACHE_MAX_ENTRIES = 1e4;
function sweepCache() {
	const now = Date.now();
	for (const [k, v] of cacheStore) if (v.staleUntil < now) cacheStore.delete(k);
	if (cacheStore.size > CACHE_MAX_ENTRIES) {
		let drop = cacheStore.size - CACHE_MAX_ENTRIES;
		for (const k of cacheStore.keys()) {
			if (drop-- <= 0) break;
			cacheStore.delete(k);
		}
	}
}
const cacheSweeper = setInterval(sweepCache, 6e4);
if (typeof cacheSweeper === "object" && cacheSweeper !== null && "unref" in cacheSweeper && typeof cacheSweeper.unref === "function") cacheSweeper.unref();
/** Drop cached entries carrying any of the given tags (soft→delete here; the
* Rust TieredCache implements the stale-serve semantics, §11.3). */
function invalidateTag(...tags) {
	let n = 0;
	for (const [key, entry] of cacheStore) if (entry.tags.some((t) => tags.includes(t))) {
		cacheStore.delete(key);
		n++;
	}
	return n;
}
/**
* The explicit form of the `'use cache'` directive (§9.3): wraps an async fn
* with an in-memory entry keyed by `cacheKey(args, opts)`. Concurrent misses
* for the same key single-flight; `swr` serves stale then refreshes in the
* background.
*/
function cache(fn, opts = {}) {
	const ttlMs = (opts.ttl ?? 0) * 1e3;
	const swrMs = (opts.swr ?? 0) * 1e3;
	const tagsFor = (args) => {
		const t = opts.tags;
		const raw = typeof t === "function" ? t(...args) : t;
		return Array.isArray(raw) ? raw.filter((x) => typeof x === "string") : [];
	};
	const wrapped = async (...args) => {
		const callTags = tagsFor(args);
		const key = cacheKey(args, {
			...opts,
			tags: callTags
		});
		if (hostOpAvailable()) {
			const host = await hostGet(key);
			if (host !== void 0) return host;
		}
		const now = Date.now();
		const hit = cacheStore.get(key);
		if (hit) {
			if (now < hit.expiresAt) return hit.value;
			if (swrMs > 0 && now < hit.staleUntil) {
				refresh(key, args, callTags);
				return hit.value;
			}
			cacheStore.delete(key);
		}
		const inflight = cachePending.get(key);
		if (inflight) return await inflight;
		return await runAndStore(key, args, callTags);
	};
	/** §9.3: `cache_get` through the host bridge. `undefined` means "not
	*  cached" — a null reply, or no host, or a host error (the in-process
	*  Map then runs). */
	async function hostGet(key) {
		try {
			const v = await globalThis.__oxide_op_request("cache_get", { key });
			if (v === null || v === void 0) return void 0;
			if (typeof v.body_b64 === "string" && v.body_b64.length > 0) {
				const bin = atob(v.body_b64);
				const bytes = new Uint8Array(bin.length);
				for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
				return decodeOdf(bytes);
			}
			return v.value;
		} catch {
			return;
		}
	}
	/** §9.3: `cache_set` through the host bridge. Best-effort: a failure
	*  leaves the in-process Map as the only copy. */
	function hostSet(key, value, tags) {
		if (!hostOpAvailable()) return;
		let body_b64 = "";
		try {
			body_b64 = bytesToBase64(encodeOdf(value));
		} catch (e) {
			console.warn?.("oxide: cache value is not ODF-encodable; storing JSON", e);
			const json = JSON.stringify(value);
			body_b64 = bytesToBase64(new TextEncoder().encode(json));
		}
		globalThis.__oxide_op_request("cache_set", {
			key,
			body_b64,
			tags,
			ttl: opts.ttl ?? 0,
			swr: opts.swr
		}).catch(() => {});
	}
	const runAndStore = async (key, args, tags) => {
		const p = (async () => {
			try {
				const value = await fn(...args);
				cacheStore.set(key, {
					value,
					expiresAt: Date.now() + ttlMs,
					staleUntil: Date.now() + ttlMs + swrMs,
					tags: [...tags]
				});
				if (cacheStore.size > CACHE_MAX_ENTRIES) sweepCache();
				hostSet(key, value, tags);
				return value;
			} finally {
				cachePending.delete(key);
			}
		})();
		cachePending.set(key, p);
		return await p;
	};
	const refresh = (key, args, tags) => {
		if (cachePending.has(key)) return cachePending.get(key);
		return runAndStore(key, args, tags);
	};
	return Object.assign(wrapped, {
		$$oxideCache: true,
		opts
	});
}
/**
* Inside a `'use cache'` fn: attach tags to the cache entry (§9.3). The
* compiler pass threads these to the cache wrapper; standalone it returns a
* documented marker.
*/
function cacheTag(...tags) {
	return { __oxideCacheTag: tags };
}
/** Inside a `'use cache'` fn: override ttl/swr for this entry (§9.3). */
function cacheLife(life) {
	return { __oxideCacheLife: life };
}
if (typeof globalThis.__oxide_cache !== "function") globalThis.__oxide_cache = (fn, _module, _exportName, opts) => {
	return cache(fn, {
		ttl: opts.life?.ttl,
		swr: opts.life?.swr,
		tags: opts.tags
	});
};
const globalRegistry = globalThis;
const initialRegistry = Object.create(null);
globalRegistry.__oxide_fn_registry = globalRegistry.__oxide_fn_registry ?? initialRegistry;
/** Live id → fn registry (ids are computed by the Rust build, §9.4).
*  Parameters are typed `never[]` (contravariant bottom) so any concrete
*  function signature is assignable to it, matching how a heterogeneous
*  registry actually behaves at runtime — the caller resolves the exact
*  parameter shape per-id. */
const __oxide_fn_registry = globalRegistry.__oxide_fn_registry;
/** Look up a registered server fn by its build id. */
function getServerFn(id) {
	return __oxide_fn_registry[id];
}
if (typeof globalThis.__oxide_register_fn !== "function") globalThis.__oxide_register_fn = (id, fn) => {
	__oxide_fn_registry[id] = fn;
};
let anonymousFnCounter = 0;
/**
* Tag + register a server function (§9.4). In app code the compiler rewrites
* `serverFn(fn)` to `serverFn('<stable id>', fn)`; the runtime registers the
* fn so `POST /_oxide/fn/<id>` can dispatch to it. The returned fn remains
* directly callable in-process (tests, SSR).
*/
function serverFn(fnOrId, maybeFn) {
	const isIdFirst = typeof fnOrId === "string";
	const fn = isIdFirst ? maybeFn : fnOrId;
	if (typeof fn !== "function") throw new TypeError("serverFn(fn) or serverFn(id, fn) — a function is required");
	const id = isIdFirst ? fnOrId : fn.name || `anonymous-${++anonymousFnCounter}`;
	const tagged = fn;
	tagged.$$oxideServerFn = true;
	tagged.id = id;
	__oxide_fn_registry[id] = tagged;
	return tagged;
}
/** All revalidate calls recorded in order (inspector/tests). */
const _revalidateLog = [];
/**
* Collectors for cache invalidation (§11.3). In actions, prefer
* `event.revalidate`; this standalone form records into `_revalidateLog` for
* the inspector and tests.
*/
const revalidate = {
	tag(tag) {
		const ev = {
			type: "tag",
			tag
		};
		_revalidateLog.push(ev);
		return ev;
	},
	path(path) {
		const ev = {
			type: "path",
			path
		};
		_revalidateLog.push(ev);
		return ev;
	}
};
/**
* §16.1: build a `srcset` attribute string from a base image URL and a set
* of widths. Pairs with the `/_oxide/image` endpoint or a pre-generated
* `?w=` variant set. A leading `./` or `/` URL is passed through verbatim;
* `signImageUrl` (below) appends a signature when the project uses signed
* image URLs.
*
* @example
*   <img
*     src="/_oxide/image?src=/hero.png&w=1280"
*     srcset={srcset([640, 1024, 1280, 1920], '/_oxide/image?src=/hero.png')}
*     sizes="(min-width: 1280px) 1280px, 100vw"
*     alt="Hero"
*   />
*/
function srcset(widths, baseUrl) {
	const seen = /* @__PURE__ */ new Set();
	const sorted = [...widths].filter((w) => {
		if (!Number.isFinite(w) || w <= 0) return false;
		if (seen.has(w)) return false;
		seen.add(w);
		return true;
	}).sort((a, b) => a - b);
	const sep = baseUrl.includes("?") ? "&" : "?";
	return sorted.map((w) => `${baseUrl}${sep}w=${w} ${w}w`).join(", ");
}
export { BOUNDARY_CONTENT_ATTR, BOUNDARY_FALLBACK_ATTR, OXIDE_TAG, OxideHttpError, OxideRedirect, __clearContentCache, __oxide_fn_registry, _revalidateLog, beginBoundaryCapture, boundaryContent, boundaryFallback, cache, cacheKey, cacheLife, cacheTag, defer, deferredIdOf, defineAction, defineLoad, defineMeta, defineSocket, drainBoundaries, endBoundaryCapture, error, getCollection, getEntry, getServerFn, invalidateTag, redirect, registerBoundary, revalidate, serverFn, srcset };

//# sourceMappingURL=server.js.map