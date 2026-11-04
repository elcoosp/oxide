// src/test.ts
/**
* Wrap a request handler (the same handler shape the Rust pipeline dispatches
* to) into a fetch-like test client:
*
*   const app = createTestApp((req) => new Response('hi'))
*   const res = await app.fetch({ method: 'POST', url: '/?/comment', body: 'x' })
*
* `Request` instances pass through untouched; strings become GET requests;
* object input is normalized into a `Request` (a body without an explicit
* method defaults to POST).
*/
function createTestApp(handler) {
	return { async fetch(input) {
		return await handler(normalize(input));
	} };
}
function normalize(input) {
	if (typeof input === "string") return new Request(input);
	if (typeof input.arrayBuffer === "function" && input instanceof Request) return input;
	const spec = input;
	const headers = spec.headers instanceof Headers ? spec.headers : new Headers(spec.headers);
	const method = (spec.method ?? (spec.body !== void 0 ? "POST" : "GET")).toUpperCase();
	const init = {
		method,
		headers,
		body: method === "GET" || method === "HEAD" ? void 0 : spec.body
	};
	if (init.body instanceof ReadableStream) init.duplex = "half";
	return new Request(spec.url, init);
}
export { createTestApp };

//# sourceMappingURL=test.js.map