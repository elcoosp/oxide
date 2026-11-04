import { bytesToBase64 } from "./bytes.js";
// src/odf-encode.ts
const TAG = {
	NULL: 0,
	UNDEFINED: 1,
	BOOL: 2,
	NUMBER: 3,
	BIGINT: 4,
	STRING: 5,
	DATE: 6,
	URL: 7,
	REGEXP: 8,
	ARRAY: 9,
	OBJECT: 10,
	MAP: 11,
	SET: 12,
	TYPED_ARRAY: 13,
	ERROR: 14,
	REF: 15,
	DEFERRED: 16
};
/**
* Typed-array kind byte → the matching global constructor, in the exact order
* `crates/odf/src/wire.rs` declares (`TypedArrayKind`): 0 = I8 … 10 = BigUint64.
*/
const TYPED_ARRAY_KINDS = [
	{
		ctor: globalThis.Int8Array,
		kind: 0
	},
	{
		ctor: globalThis.Uint8Array,
		kind: 1
	},
	{
		ctor: globalThis.Uint8ClampedArray,
		kind: 2
	},
	{
		ctor: globalThis.Int16Array,
		kind: 3
	},
	{
		ctor: globalThis.Uint16Array,
		kind: 4
	},
	{
		ctor: globalThis.Int32Array,
		kind: 5
	},
	{
		ctor: globalThis.Uint32Array,
		kind: 6
	},
	{
		ctor: globalThis.Float32Array,
		kind: 7
	},
	{
		ctor: globalThis.Float64Array,
		kind: 8
	},
	{
		ctor: globalThis.BigInt64Array,
		kind: 9
	},
	{
		ctor: globalThis.BigUint64Array,
		kind: 10
	}
];
function typedArrayKindOf(v) {
	for (const { ctor, kind } of TYPED_ARRAY_KINDS) if (typeof ctor === "function" && v instanceof ctor) return kind;
}
function varu(out, n) {
	n = n >>> 0;
	while (n >= 128) {
		out.push(n & 127 | 128);
		n >>>= 7;
	}
	out.push(n);
}
function utf8(s) {
	return Array.from(new TextEncoder().encode(s));
}
const UTF8_ENCODER = new TextEncoder();
/**
* B-14: sort object keys by UTF-8 byte order, matching Rust's
* `BTreeMap<String, _>`. JS `.sort()` compares UTF-16 code units, which
* diverges for keys mixing BMP (U+E000–U+FFFF) with surrogate pairs.
*/
function byUtf8(a, b) {
	const ab = UTF8_ENCODER.encode(a);
	const bb = UTF8_ENCODER.encode(b);
	const n = Math.min(ab.length, bb.length);
	for (let i = 0; i < n; i++) {
		const a = ab[i];
		const b = bb[i];
		if (a === void 0 || b === void 0) continue;
		if (a !== b) return a - b;
	}
	return ab.length - bb.length;
}
/** varu(len) + UTF-8 bytes — the STRING body shared by several tags. */
function writeString(out, s) {
	const bytes = utf8(s);
	varu(out, bytes.length);
	out.push(...bytes);
}
function writeValue(out, v) {
	if (v === null) {
		out.push(TAG.NULL);
		return;
	}
	if (v === void 0) {
		out.push(TAG.UNDEFINED);
		return;
	}
	if (typeof v === "boolean") {
		out.push(TAG.BOOL, v ? 1 : 0);
		return;
	}
	if (typeof v === "number") {
		out.push(TAG.NUMBER);
		const buf = /* @__PURE__ */ new ArrayBuffer(8);
		new DataView(buf).setFloat64(0, v, true);
		out.push(...new Uint8Array(buf));
		return;
	}
	if (typeof v === "string") {
		out.push(TAG.STRING);
		const bytes = utf8(v);
		varu(out, bytes.length);
		out.push(...bytes);
		return;
	}
	if (Array.isArray(v)) {
		out.push(TAG.ARRAY);
		varu(out, v.length);
		for (const item of v) writeValue(out, item);
		return;
	}
	if (typeof v === "bigint") {
		const dec = v.toString(10);
		out.push(TAG.BIGINT);
		const bytes = utf8(dec);
		varu(out, bytes.length);
		out.push(...bytes);
		return;
	}
	if (typeof v === "object") {
		if (v instanceof Error) {
			out.push(TAG.ERROR);
			writeString(out, typeof v.name === "string" ? v.name : "Error");
			writeString(out, typeof v.message === "string" ? v.message : "");
			if (typeof v.stack === "string") {
				out.push(1);
				writeString(out, v.stack);
			} else out.push(0);
			return;
		}
		const marker = v;
		if (typeof marker.__oxide_ref === "number") {
			out.push(TAG.REF);
			varu(out, marker.__oxide_ref);
			return;
		}
		if (marker.__oxide_deferred !== void 0 && typeof marker.__oxide_deferred_id === "number") {
			out.push(TAG.DEFERRED);
			varu(out, marker.__oxide_deferred_id);
			return;
		}
		if (v instanceof Date) {
			const ms = v.getTime();
			if (!Number.isFinite(ms)) throw new TypeError("odf: cannot encode an Invalid Date");
			out.push(TAG.DATE);
			const buf = /* @__PURE__ */ new ArrayBuffer(8);
			new DataView(buf).setBigInt64(0, BigInt(Math.trunc(ms)), true);
			out.push(...new Uint8Array(buf));
			return;
		}
		if (v instanceof Map) {
			out.push(TAG.MAP);
			varu(out, v.size);
			for (const [k, val] of v) {
				writeValue(out, k);
				writeValue(out, val);
			}
			return;
		}
		if (v instanceof Set) {
			out.push(TAG.SET);
			varu(out, v.size);
			for (const item of v) writeValue(out, item);
			return;
		}
		if (typeof URL !== "undefined" && v instanceof URL) {
			const s = v.toString();
			out.push(TAG.URL);
			const bytes = utf8(s);
			varu(out, bytes.length);
			out.push(...bytes);
			return;
		}
		if (v instanceof RegExp) {
			const source = v.source;
			const flags = v.flags;
			out.push(TAG.REGEXP);
			const sb = utf8(source);
			varu(out, sb.length);
			out.push(...sb);
			const fb = utf8(flags);
			varu(out, fb.length);
			out.push(...fb);
			return;
		}
		const taKind = typedArrayKindOf(v);
		if (taKind !== void 0) {
			const view = v;
			const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
			out.push(TAG.TYPED_ARRAY);
			out.push(taKind);
			varu(out, bytes.length);
			out.push(...bytes);
			return;
		}
		const keys = Object.keys(v).filter((k) => k !== "__proto__" && k !== "constructor").sort(byUtf8);
		out.push(TAG.OBJECT);
		varu(out, keys.length);
		for (const k of keys) {
			const kb = utf8(k);
			varu(out, kb.length);
			out.push(...kb);
			writeValue(out, v[k]);
		}
		return;
	}
	throw new TypeError(`odf: cannot encode ${typeof v}`);
}
/** True when `v` is inside the encoder's JSON subset (no cycles, no exotic types). */
function isOdfEncodable(v, depth = 0, seen = /* @__PURE__ */ new Set()) {
	if (depth > 100) return false;
	if (v === null || v === void 0) return true;
	const t = typeof v;
	if (t === "boolean" || t === "number" || t === "string" || t === "bigint") return true;
	if (t !== "object") return false;
	const o = v;
	if (seen.has(o)) return false;
	seen.add(o);
	let ok = true;
	if (Array.isArray(v)) ok = v.every((x) => isOdfEncodable(x, depth + 1, seen));
	else if (v instanceof Error) ok = true;
	else if (v instanceof Date) ok = Number.isFinite(v.getTime());
	else if (v instanceof Map) {
		ok = true;
		for (const [k, val] of v) if (!isOdfEncodable(k, depth + 1, seen) || !isOdfEncodable(val, depth + 1, seen)) {
			ok = false;
			break;
		}
	} else if (v instanceof Set) {
		ok = true;
		for (const item of v) if (!isOdfEncodable(item, depth + 1, seen)) {
			ok = false;
			break;
		}
	} else if (typeof URL !== "undefined" && v instanceof URL) ok = true;
	else if (v instanceof RegExp) ok = true;
	else if (typedArrayKindOf(o) !== void 0) ok = true;
	else if (Object.getPrototypeOf(o) === Object.prototype || Object.getPrototypeOf(o) === null) ok = Object.values(v).every((x) => isOdfEncodable(x, depth + 1, seen));
	else ok = false;
	seen.delete(o);
	return ok;
}
/** Encode a JSON-subset value to ODF bytes (no preamble). */
function encodeOdf(value) {
	const out = [
		79,
		68,
		70,
		49
	];
	varu(out, 0);
	writeValue(out, value);
	return new Uint8Array(out);
}
/**
* PERF-01: encode island `props` for `data-ox-props`. Returns base64 of the
* ODF bytes when `props` is inside the JSON subset; otherwise base64 of
* `JSON.stringify(props)` — the client's `decodeIslandProps` accepts both
* (it sniffs the `ODF1` magic, falling back to JSON). So a prop carrying a
* Date/Map/cycle still hydrates; only the common case pays the ODF path.
*/
function encodePropsB64(props) {
	const toB64 = (bytes) => bytesToBase64(bytes);
	if (isOdfEncodable(props)) try {
		return toB64(encodeOdf(props));
	} catch {}
	return toB64(new TextEncoder().encode(JSON.stringify(props)));
}
export { encodeOdf, encodePropsB64, isOdfEncodable };

//# sourceMappingURL=odf-encode.js.map