// src/client/defer-glue.ts
/** A promise-like the client adapter can await; resolves on ox:swap. */
var DeferredClient = class {
	status = "pending";
	value;
	error;
	promise;
	#resolveUnknown;
	#rejectUnknown;
	constructor() {
		let resolveUnknown;
		let rejectUnknown;
		const p = new Promise((res, rej) => {
			resolveUnknown = res;
			rejectUnknown = rej;
		});
		this.promise = p;
		this.#resolveUnknown = resolveUnknown;
		this.#rejectUnknown = rejectUnknown;
		this.promise.catch(() => {});
	}
	resolve(value) {
		if (this.status !== "pending") return;
		this.status = "resolved";
		this.value = value;
		this.#resolveUnknown(value);
	}
	reject(error) {
		if (this.status !== "pending") return;
		this.status = "rejected";
		this.error = error;
		this.#rejectUnknown(error instanceof Error ? error : /* @__PURE__ */ new Error(`deferred slot rejected: ${String(error)}`));
	}
	then = (onOk, onErr) => this.promise.then(onOk, onErr);
	catch = (onErr) => this.promise.catch(onErr);
};
function isSwapEvent(e) {
	if (typeof e !== "object" || e === null) return false;
	if (!("detail" in e)) return false;
	const d = e.detail;
	if (typeof d !== "object" || d === null) return false;
	return "id" in d && typeof d.id === "number";
}
function attachDeferGlue(options = {}) {
	const doc = options.doc ?? globalThis.document;
	const eventName = options.eventName ?? "ox:swap";
	const registry = /* @__PURE__ */ new Map();
	/** id → desired text. Populated on resolve; consumed by the marker
	*  MutationObserver that reconciles any later clobber (React remount,
	*  framework re-render, external override). */
	const reconciled = /* @__PURE__ */ new Map();
	/**
	* §7.1: when a slot resolves, if the page contains a
	* `[data-deferred-value="<id>"]` marker, write the resolved value into
	* it. This lets a page that doesn't use framework idioms (React `use()`,
	* Vue ref, Svelte await block, Solid resource) still *see* the streamed
	* value — the last-mile wiring that turns "the settle script landed" into
	* "the page shows the label."
	*
	* Convention (matching the smoke-app `/deferred` fixture):
	*   `value = { label: <string> }`  ->  marker.textContent = value.label
	*   `value = <string>`             ->  marker.textContent = value
	*   `value = anything else`        ->  marker.textContent = JSON.stringify(value)
	* Opt-in per element: no marker, no work. Multiple markers per id (rare)
	* all receive the same value.
	*/
	const updateMarker = (id, value) => {
		if (!doc || typeof doc.querySelectorAll !== "function") return;
		const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`);
		if (!els || els.length === 0) return;
		const el = els[0];
		if (!el) return;
		if (value && typeof value === "object" && typeof value.label === "string") el.textContent = value.label;
		else if (typeof value === "string") el.textContent = value;
		else el.textContent = JSON.stringify(value);
	};
	/** Compute the text a marker should show for a resolved value. Mirrors
	*  `updateMarker`'s convention so a retry can compare and skip no-ops. */
	const desiredText = (value) => {
		if (value && typeof value === "object" && typeof value.label === "string") return value.label;
		if (typeof value === "string") return value;
		return JSON.stringify(value);
	};
	/**
	* Reconcile a marker write across several animation frames. Needed because
	* React's hydration can (a) error with #418 on a text mismatch, and (b)
	* respond by **replacing the marker element** — not just its text — so a
	* one-shot write against the pre-hydration element is lost. React may also
	* do its work in a microtask that runs *after* the first rAF, so a single
	* reapply on frame 1 can still be too early.
	*
	* Strategy: run for the full 30-frame window (≈500ms at 60fps) with **no
	* early exit on stability**. An earlier attempt stopped after 3 stable
	* frames — that heuristic missed React #418's clobber, which lands a few
	* frames *after* the glue settles. On non-React pages, the 30 no-op frames
	* cost ~0.1ms each — negligible. The persistent MutationObserver
	* (`attachDeferGlue`) is belt-and-braces for anything later than 500ms.
	*/
	const scheduleMarkerReapply = (id, value) => {
		const raf = globalThis.requestAnimationFrame;
		if (typeof raf !== "function") return;
		const desired = desiredText(value);
		let attempts = 0;
		const MAX_ATTEMPTS = 30;
		const tick = () => {
			attempts++;
			if (dead) return;
			if (attempts > MAX_ATTEMPTS) return;
			if (!doc || typeof doc.querySelectorAll !== "function") return;
			const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`);
			const el = els && els.length > 0 ? els[0] : void 0;
			if (el && el.textContent !== desired) el.textContent = desired;
			raf(tick);
		};
		raf(tick);
	};
	/**
	* Resolve (or reject) one slot from a settle value, and update any
	* matching `[data-deferred-value]` marker. A rejected defer is
	* serialized as `{ __oxide_deferred_error: "..." }` and rejects the
	* slot rather than resolving it to a shape the page did not expect.
	* Called from every resolution path: per-settle scripts, batched
	* scripts, `ox:swap` events, and the programmatic `glue.resolve`/
	* `glue.resolveAll` API.
	*/
	const resolveSlot = (id, value) => {
		const slot = registry.get(id);
		if (!slot) return;
		if (value && typeof value === "object" && "__oxide_deferred_error" in value) {
			slot.reject(new Error(String(value.__oxide_deferred_error)));
			return;
		}
		const wasPending = slot.status === "pending";
		slot.resolve(value);
		if (wasPending) {
			reconciled.set(id, desiredText(value));
			updateMarker(id, value);
			scheduleMarkerReapply(id, value);
		}
	};
	let dead = false;
	if (!doc) throw new TypeError("attachDeferGlue: no document (pass one in non-browser tests)");
	const onSwap = (e) => {
		if (!isSwapEvent(e)) return;
		const detail = e.detail;
		options.onSwap?.(detail);
		if (detail.error !== void 0 && detail.error !== null) {
			registry.get(detail.id)?.reject(detail.error);
			return;
		}
		resolveSlot(detail.id, detail.value);
	};
	doc.addEventListener(eventName, onSwap);
	const resolveFromScripts = () => {
		if (typeof doc.querySelectorAll !== "function") return;
		const scripts = doc.querySelectorAll("script[type=\"application/oxide-deferred\"]");
		for (let i = 0; i < scripts.length; i++) {
			const script = scripts[i];
			if (script === void 0) continue;
			const raw = script.textContent ?? "";
			if (raw.length === 0) continue;
			let parsed;
			try {
				parsed = JSON.parse(raw);
			} catch {
				continue;
			}
			const dataId = script.getAttribute?.("data-id");
			if (dataId) {
				const id = Number(dataId);
				if (Number.isFinite(id)) resolveSlot(id, parsed);
				continue;
			}
			for (const [idStr, value] of Object.entries(parsed)) {
				const id = Number(idStr);
				if (!Number.isFinite(id)) continue;
				resolveSlot(id, value);
			}
		}
	};
	let observer = null;
	const MO = globalThis.MutationObserver;
	if (typeof MO === "function" && doc.documentElement !== void 0) {
		observer = new MO((mutations) => {
			if (!dead && doc && typeof doc.querySelectorAll === "function") for (const [id, desired] of reconciled) {
				const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`);
				const el = els && els.length > 0 ? els[0] : void 0;
				if (el && el.textContent !== desired) el.textContent = desired;
			}
			for (const m of mutations) {
				const added = m.addedNodes;
				if (!added) continue;
				for (let i = 0; i < added.length; i++) {
					const node = added[i];
					if (node.tagName === "SCRIPT" && node.getAttribute?.("type") === "application/oxide-deferred") {
						resolveFromScripts();
						return;
					}
				}
			}
		});
		observer.observe(doc.documentElement, {
			childList: true,
			subtree: true,
			characterData: true
		});
	}
	resolveFromScripts();
	const onReady = () => resolveFromScripts();
	if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", onReady);
	return {
		registry,
		resolve(id, value) {
			resolveSlot(id, value);
		},
		resolveAll(values) {
			for (const [idStr, value] of Object.entries(values)) {
				const id = Number(idStr);
				if (!Number.isFinite(id)) continue;
				resolveSlot(id, value);
			}
		},
		reject(id, error) {
			registry.get(id)?.reject(error);
		},
		wire(data) {
			wireDeferreds(data, {
				registry,
				dead
			});
			resolveFromScripts();
		},
		dispose() {
			observer?.disconnect();
			observer = null;
			dead = true;
			doc.removeEventListener(eventName, onSwap);
			doc.removeEventListener("DOMContentLoaded", onReady);
			for (const [id, slot] of registry) if (slot.status === "pending") slot.reject(/* @__PURE__ */ new Error(`deferred slot ${id}: glue disposed before swap`));
		}
	};
}
const isPlaceholder = (v) => typeof v === "object" && v !== null && typeof v.__deferred === "number" && Object.keys(v).length === 1;
/**
* Walk `data` (which came from the ODF decoder) and replace every
* `{__deferred: id}` placeholder with a [DeferredClient] registered under its
* id. Mutates containers in place; cycle-safe; depth-capped like the decoder.
*/
function wireDeferreds(data, options = {}) {
	walk(data, options.registry ?? /* @__PURE__ */ new Map(), 1, /* @__PURE__ */ new WeakSet());
	return data;
}
function walk(value, registry, depth, seen, dead = false) {
	if (depth > 512 || value === null || typeof value !== "object") return;
	if (isPlaceholder(value)) return;
	if (seen.has(value)) return;
	seen.add(value);
	if (value instanceof Map) {
		for (const [k, v] of value) if (isPlaceholder(v)) value.set(k, slotFor(v.__deferred, registry, dead));
		else walk(v, registry, depth + 1, seen, dead);
		return;
	}
	if (value instanceof Set) {
		const items = [...value];
		value.clear();
		for (const item of items) value.add(isPlaceholder(item) ? slotFor(item.__deferred, registry, dead) : (walk(item, registry, depth + 1, seen, dead), item));
		return;
	}
	const obj = value;
	for (const key of Object.keys(obj)) {
		const v = obj[key];
		if (isPlaceholder(v)) obj[key] = slotFor(v.__deferred, registry, dead);
		else walk(v, registry, depth + 1, seen, dead);
	}
}
function slotFor(id, registry, dead = false) {
	let slot = registry.get(id);
	if (!slot) {
		slot = new DeferredClient();
		if (dead) slot.reject(/* @__PURE__ */ new Error(`deferred slot ${id}: glue disposed before wire`));
		registry.set(id, slot);
	}
	return slot;
}
export { DeferredClient, attachDeferGlue, wireDeferreds };

//# sourceMappingURL=client-defer-glue.js.map