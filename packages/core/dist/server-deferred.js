// src/server/deferred.ts
function resolveDeferredPlaceholders(value, deferred, depth = 1, seen = /* @__PURE__ */ new WeakSet()) {
	if (depth > 512 || value === null || typeof value !== "object") return value;
	if (seen.has(value)) return value;
	seen.add(value);
	if (Array.isArray(value)) {
		for (let i = 0; i < value.length; i++) value[i] = resolveDeferredPlaceholders(value[i], deferred, depth + 1, seen);
		return value;
	}
	const maybePlaceholder = value;
	if (typeof maybePlaceholder.__deferred === "number" && Object.keys(value).length === 1) {
		const id = String(maybePlaceholder.__deferred);
		if (deferred && Object.prototype.hasOwnProperty.call(deferred, id)) return deferred[id];
		return value;
	}
	if (value instanceof Map) {
		for (const [k, v] of value) value.set(k, resolveDeferredPlaceholders(v, deferred, depth + 1, seen));
		return value;
	}
	if (value instanceof Set) return new Set([...value].map((v) => resolveDeferredPlaceholders(v, deferred, depth + 1, seen)));
	const obj = value;
	for (const k of Object.keys(obj)) {
		if (k === "__proto__" || k === "constructor") continue;
		obj[k] = resolveDeferredPlaceholders(obj[k], deferred, depth + 1, seen);
	}
	return obj;
}
export { resolveDeferredPlaceholders };

//# sourceMappingURL=server-deferred.js.map