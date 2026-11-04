// src/fn.ts
/**
* Call the server function named by `id` with `args`. Returns the resolved
* value, or throws an `Error` whose message is the server's diagnostic.
*
* The `content-type` is `application/oxide-fn` — the compiler's stub used
* this marker and the endpoint matches on it, so a project can tell a
* server-fn POST from a form action POST in a log.
*/
async function __oxide_call_fn(id, args) {
	const r = await fetch(`/_oxide/fn/${encodeURIComponent(id)}`, {
		method: "POST",
		headers: { "content-type": "application/oxide-fn" },
		body: JSON.stringify(args)
	});
	const text = await r.text();
	let parsed;
	try {
		parsed = JSON.parse(text);
	} catch {
		throw new Error(`server fn ${id} returned ${r.status} with a non-JSON body: ${text.slice(0, 120)}`);
	}
	if (parsed.ok) return parsed.data;
	const e = new Error(parsed.message);
	e.code = parsed.code;
	throw e;
}
export { __oxide_call_fn };

//# sourceMappingURL=fn.js.map