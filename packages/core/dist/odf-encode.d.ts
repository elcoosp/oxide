/** True when `v` is inside the encoder's JSON subset (no cycles, no exotic types). */
export declare function isOdfEncodable(v: unknown, depth?: number, seen?: Set<object>): boolean;
/** Encode a JSON-subset value to ODF bytes (no preamble). */
export declare function encodeOdf(value: unknown): Uint8Array;
/**
 * PERF-01: encode island `props` for `data-ox-props`. Returns base64 of the
 * ODF bytes when `props` is inside the JSON subset; otherwise base64 of
 * `JSON.stringify(props)` — the client's `decodeIslandProps` accepts both
 * (it sniffs the `ODF1` magic, falling back to JSON). So a prop carrying a
 * Date/Map/cycle still hydrates; only the common case pays the ODF path.
 */
export declare function encodePropsB64(props: unknown): string;
//# sourceMappingURL=odf-encode.d.ts.map