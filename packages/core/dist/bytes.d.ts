/**
 * Base64-encode a UTF-8 string. Uses `btoa` in the browser (with a UTF-8
 * round-trip so non-ASCII survives) and `Buffer` in Node.
 */
export declare function toBase64(s: string): string;
/** P-09: base64-encode raw bytes (browser `btoa`, Node `Buffer`). */
export declare function bytesToBase64(bytes: Uint8Array): string;
/**
 * Base64-decode to raw bytes. Used for island props (base64-encoded JSON
 * or ODF) and any protocol payload.
 */
export declare function fromBase64(b64: string): Uint8Array;
//# sourceMappingURL=bytes.d.ts.map