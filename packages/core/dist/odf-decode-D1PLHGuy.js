// src/client/odf-decode.ts
/** Maximum payload size (matches Rust `MAX_BYTES`). */
const MAX_BYTES = 268435456;
/** Maximum nesting depth (matches Rust `MAX_DEPTH`). */
const MAX_DEPTH = 512;
/** Maximum shared-slot table entries (matches Rust `MAX_REFS`). */
const MAX_REFS = 1 << 20;
/** Largest ECMAScript Date time value (±8.64e15 ms). */
const MAX_DATE_MS = 864e13;
const MAGIC = 1329874481;
/** Error thrown by the decoder; `code` is a stable ODF-NNN diagnostic code. */
var OdfError = class extends Error {
	code;
	constructor(code, message) {
		super(`ODF decode failed [${code}]: ${message}`);
		this.name = "OdfError";
		this.code = code;
	}
};
const TYPED_CTORS = [
	Int8Array,
	Uint8Array,
	Uint8ClampedArray,
	Int16Array,
	Uint16Array,
	Int32Array,
	Uint32Array,
	Float32Array,
	Float64Array,
	BigInt64Array,
	BigUint64Array
];
const TYPED_SIZES = [
	1,
	1,
	1,
	2,
	2,
	4,
	4,
	4,
	8,
	8,
	8
];
const TYPED_GETTERS = [
	"getInt8",
	"getUint8",
	"getUint8",
	"getInt16",
	"getUint16",
	"getInt32",
	"getUint32",
	"getFloat32",
	"getFloat64",
	"getBigInt64",
	"getBigUint64"
];
/**
* Raw (pre-`URL`-normalization) strings of decoded URLs. Rust stores
* `Value::Url(String)` verbatim; `new URL(s).href` percent-encodes e.g.
* `?q=≥`, so the to-JSON projection prefers the raw form when available.
*/
const RAW_URL = /* @__PURE__ */ new WeakMap();
/**
* Decode ODF bytes into rich JS values (see the module header for the mapping).
* Throws [OdfError] on any malformed, truncated or over-limit payload.
*/
function decodeOdf(bytes) {
	if (bytes.length > 268435456) throw new OdfError("ODF-SIZE", `payload is ${bytes.length} bytes, limit is ${MAX_BYTES}`);
	if (bytes.length < 4 || readU32BE(bytes, 0) !== MAGIC) throw new OdfError("ODF-MAGIC", "payload does not start with the b\"ODF1\" magic + version");
	const d = new Decoder(bytes);
	const n = d.varu("shared table entry count");
	if (n > 1048576) throw new OdfError("ODF-REF", `shared table declares ${n} entries, limit is ${MAX_REFS}`);
	for (let i = 0; i < n; i++) d.shared.push(d.value(1, false));
	const root = d.value(1, true);
	if (d.pos !== bytes.length) throw new OdfError("ODF-TRUNC", `trailing bytes after the root value (${bytes.length - d.pos} left)`);
	return root;
}
function readU32BE(b, at) {
	return new DataView(b.buffer, b.byteOffset, b.byteLength).getUint32(at, false);
}
var Decoder = class {
	pos = 4;
	/** Shared-slot table: preamble entries first, then root containers in pre-order. */
	shared = [];
	bytes;
	dv;
	constructor(bytes) {
		this.bytes = bytes;
		this.dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	}
	need(n, what) {
		if (this.pos + n > this.bytes.length) throw new OdfError("ODF-TRUNC", `payload ends prematurely: needed ${n} more byte(s) for ${what} at offset ${this.pos}`);
	}
	u8(what) {
		this.need(1, what);
		const byte = this.dv.getUint8(this.pos);
		this.pos++;
		return byte;
	}
	/** LEB128 varuint, at most 5 bytes; overlong/overflow → ODF-TAG (as in Rust). */
	varu(what) {
		let result = 0;
		let shift = 0;
		for (;;) {
			const b = this.u8(`varuint(${what})`);
			if (shift === 28) {
				if (b & 128) throw new OdfError("ODF-TAG", `varuint encoding exceeds 5 bytes (${what})`);
				if (b > 15) throw new OdfError("ODF-TAG", `varuint overflows u32 (${what})`);
			}
			result |= (b & 127) << shift;
			if (!(b & 128)) return result >>> 0;
			shift += 7;
		}
	}
	string(what) {
		const len = this.varu(`string(${what})`);
		this.need(len, `string(${what})`);
		const slice = this.bytes.subarray(this.pos, this.pos + len);
		this.pos += len;
		try {
			return new TextDecoder("utf-8", { fatal: true }).decode(slice);
		} catch {
			throw new OdfError("ODF-TAG", `string record is not valid UTF-8 (${what})`);
		}
	}
	/**
	* Read one record. `register=true` (root section) means every container
	* opened here takes the next shared slot, pushed BEFORE its children so
	* back-edges (cycles) resolve to the live, partially-filled object.
	*/
	value(depth, register) {
		if (depth > 512) throw new OdfError("ODF-DEPTH", `value is nested deeper than the 512-level limit`);
		const tag = this.u8("type tag");
		switch (tag) {
			case 0: return null;
			case 1: return;
			case 2: {
				const b = this.u8("boolean");
				if (b > 1) throw new OdfError("ODF-TAG", `invalid boolean byte ${b}`);
				return b === 1;
			}
			case 3: {
				this.need(8, "number");
				const v = this.dv.getFloat64(this.pos, true);
				this.pos += 8;
				return v;
			}
			case 4: {
				const s = this.string("bigint");
				if (!/^-?(0|[0-9]+)$/.test(s)) throw new OdfError("ODF-TAG", `invalid BigInt decimal string \`${s}\` (expected -?(0|[0-9]+))`);
				return BigInt(s);
			}
			case 5: return this.string("string");
			case 6: {
				this.need(8, "date");
				const ms = this.dv.getBigInt64(this.pos, true);
				this.pos += 8;
				const n = Number(ms);
				return n > MAX_DATE_MS || n < -864e13 || !Number.isFinite(n) ? n : new Date(n);
			}
			case 7: {
				const s = this.string("url");
				let u;
				try {
					u = new URL(s);
				} catch {
					throw new OdfError("ODF-TAG", `invalid URL record \`${s}\``);
				}
				RAW_URL.set(u, s);
				return u;
			}
			case 8: {
				const source = this.string("regexp source");
				const flags = this.string("regexp flags");
				try {
					return new RegExp(source, flags);
				} catch {
					throw new OdfError("ODF-TAG", `invalid RegExp /${source}/${flags}`);
				}
			}
			case 9: return this.sequence(depth, register, () => []);
			case 12: return this.sequence(depth, register, () => /* @__PURE__ */ new Set());
			case 10: {
				const obj = register ? this.openSlot({}) : {};
				const count = this.varu("object count");
				for (let i = 0; i < count; i++) {
					const key = this.string("object key");
					if (key === "__proto__" || key === "constructor") {
						this.value(depth + 1, register);
						continue;
					}
					obj[key] = this.value(depth + 1, register);
				}
				return obj;
			}
			case 11: {
				const map = register ? this.openSlot(/* @__PURE__ */ new Map()) : /* @__PURE__ */ new Map();
				const count = this.varu("map count");
				for (let i = 0; i < count; i++) {
					const k = this.value(depth + 1, register);
					const v = this.value(depth + 1, register);
					map.set(k, v);
				}
				return map;
			}
			case 13: {
				const kind = this.u8("typed-array kind");
				if (kind >= TYPED_CTORS.length) throw new OdfError("ODF-TAG", `unknown typed-array kind byte 0x${kind.toString(16).padStart(2, "0")}`);
				const size = TYPED_SIZES[kind];
				const ctor = TYPED_CTORS[kind];
				if (size === void 0 || ctor === void 0) throw new OdfError("ODF-TAG", `typed-array kind ${kind} has no size/ctor mapping`);
				const len = this.varu("typed-array length");
				if (len % size !== 0) throw new OdfError("ODF-TAG", `typed array payload length ${len} is not a multiple of the ${size}-byte element size`);
				this.need(len, "typed-array payload");
				const count = len / size;
				if (kind >= 9) {
					const out = new ctor(count);
					const get = TYPED_GETTERS[kind];
					for (let i = 0; i < count; i++) out[i] = this.dv[get](this.pos + i * size, true);
					this.pos += len;
					return out;
				}
				const out = new ctor(count);
				const get = TYPED_GETTERS[kind];
				for (let i = 0; i < count; i++) out[i] = this.dv[get](this.pos + i * size, true);
				this.pos += len;
				return out;
			}
			case 14: {
				const name = this.string("error name");
				const message = this.string("error message");
				const flag = this.u8("error stack flag");
				let stack;
				if (flag === 1) stack = this.string("error stack");
				else if (flag !== 0) throw new OdfError("ODF-TAG", `invalid stack flag byte ${flag}`);
				const Ctor = globalThis[name];
				const e = typeof Ctor === "function" && Ctor.prototype instanceof Error ? new Ctor(message) : new Error(message);
				if (e.name !== name) e.name = name;
				e.stack = stack;
				return e;
			}
			case 15: {
				const i = this.varu("ref index");
				if (i >= this.shared.length) throw new OdfError("ODF-REF", `Ref(${i}) points outside the ${this.shared.length} started slot(s)`);
				return this.shared[i];
			}
			case 16: return { __deferred: this.varu("deferred id") };
			default: throw new OdfError("ODF-TAG", `unknown type tag 0x${tag.toString(16).padStart(2, "0")}`);
		}
	}
	sequence(depth, register, make) {
		const container = register ? this.openSlot(make()) : make();
		const count = this.varu("container count");
		for (let i = 0; i < count; i++) {
			const v = this.value(depth + 1, register);
			if (container instanceof Set) container.add(v);
			else container.push(v);
		}
		return container;
	}
	/** Register a container on the shared table (pre-order slot) and return it. */
	openSlot(v) {
		if (this.shared.length >= 1048576) throw new OdfError("ODF-REF", `more than ${MAX_REFS} shared slots in one payload`);
		this.shared.push(v);
		return v;
	}
};
function debugKey(k) {
	if (typeof k === "string") return k;
	if (k === null) return "Null";
	if (k === void 0) return "Undefined";
	if (typeof k === "boolean") return `Bool(${k})`;
	if (typeof k === "number") return `Number(${Number.isInteger(k) && !Object.is(k, -0) ? `${k}.0` : String(k)})`;
	if (k instanceof Date) return `Date(${k.getTime()})`;
	if (typeof k === "bigint") return `BigInt(${k})`;
	return String(k);
}
/**
* Decode and project to a JSON-serializable shape (mirrors Rust `Value::to_json`):
* undefined→null, NaN/±Inf→null, Date→epoch ms number, BigInt→decimal string,
* URL→href, RegExp→source, Map→object with Rust-Debug-formatted keys,
* Set→array, typed array→byte array, Error→{name, message}, resolved Refs→
* null on revisit (cycles cut, like Value::Ref.to_json).
*/
function decodeOdfToJson(bytes) {
	const seen = /* @__PURE__ */ new WeakSet();
	const walk = (v) => {
		if (v === null || v === void 0) return null;
		switch (typeof v) {
			case "number":
			case "boolean":
			case "string": return v;
			case "bigint": return v.toString();
			case "object": {
				if (seen.has(v)) return null;
				seen.add(v);
				if (v instanceof Date) return v.getTime();
				if (v instanceof URL) return RAW_URL.get(v) ?? v.href;
				if (v instanceof RegExp) return v.source;
				if (v instanceof Error) return {
					name: v.name,
					message: v.message
				};
				if (ArrayBuffer.isView(v)) return Array.from(new Uint8Array(v.buffer, v.byteOffset, v.byteLength));
				if (v instanceof Map) {
					const o = {};
					for (const [k, val] of v) o[debugKey(k)] = walk(val);
					return o;
				}
				if (v instanceof Set) return [...v].map(walk);
				if (Array.isArray(v)) return v.map(walk);
				const o = {};
				for (const [k, val] of Object.entries(v)) o[k] = walk(val);
				return o;
			}
			default: return String(v);
		}
	};
	return walk(decodeOdf(bytes));
}
export { decodeOdf as a, OdfError as i, MAX_DEPTH as n, decodeOdfToJson as o, MAX_REFS as r, MAX_BYTES as t };

//# sourceMappingURL=odf-decode-D1PLHGuy.js.map