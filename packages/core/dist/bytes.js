// src/bytes.ts
function globals() {
	return globalThis;
}
/**
* Base64-encode a UTF-8 string. Uses `btoa` in the browser (with a UTF-8
* round-trip so non-ASCII survives) and `Buffer` in Node.
*/
function toBase64(s) {
	const g = globals();
	if (typeof g.btoa === "function") return g.btoa(unescape(encodeURIComponent(s)));
	if (g.Buffer) return g.Buffer.from(s, "utf8").toString("base64");
	throw new TypeError("toBase64: no base64 encoder available");
}
/**
* P-09: convert a byte array to a binary string in linear time. A per-byte
* `bin += String.fromCharCode(b)` loop is O(n²) in V8 (each concat re-copies);
* chunked `String.fromCharCode(...chunk)` is linear.
*/
function bytesToBinary(bytes) {
	const CHUNK = 32768;
	let out = "";
	for (let i = 0; i < bytes.length; i += CHUNK) out += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
	return out;
}
/** P-09: base64-encode raw bytes (browser `btoa`, Node `Buffer`). */
function bytesToBase64(bytes) {
	const g = globals();
	if (typeof g.btoa === "function") return g.btoa(bytesToBinary(bytes));
	if (g.Buffer) return g.Buffer.from(bytes).toString("base64");
	throw new TypeError("bytesToBase64: no base64 encoder available");
}
/**
* Base64-decode to raw bytes. Used for island props (base64-encoded JSON
* or ODF) and any protocol payload.
*/
function fromBase64(b64) {
	const g = globals();
	if (typeof g.atob === "function") {
		const bin = g.atob(b64);
		const bytes = new Uint8Array(bin.length);
		for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
		return bytes;
	}
	if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, "base64"));
	throw new TypeError("fromBase64: no base64 decoder available");
}
export { bytesToBase64, fromBase64, toBase64 };

//# sourceMappingURL=bytes.js.map