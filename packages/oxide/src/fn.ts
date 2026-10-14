// §9.4 server functions — the client-side half.
//
// The compiler rewrites `export const add = serverFn(async (…) => {…})` in
// a `*.server.ts` module to a stub that calls this module:
//
//     import { __oxide_call_fn } from 'oxide/fn';
//     export const add = (async (...args) => __oxide_call_fn('<id>', args));
//
// `__oxide_call_fn` POSTs the encoded args to `/_oxide/fn/<id>` and decodes
// the response. The wire format is ODF (§9.5), but this module only needs
// the shape: an array of args in, a value out. The encoding is done here
// rather than via the ODF encoder, which is a Rust op only available inside
// the worker — a browser has no access to it. A JSON fallback is enough
// for the common case and matches what the server currently accepts.

/** The response body shape the endpoint returns for a resolved fn. */
interface FnOk {
  ok: true
  data: unknown
}
/** The response body shape for a rejected fn. */
interface FnErr {
  ok: false
  code: string
  message: string
}

/**
 * Call the server function named by `id` with `args`. Returns the resolved
 * value, or throws an `Error` whose message is the server's diagnostic.
 *
 * The `content-type` is `application/oxide-fn` — the compiler's stub used
 * this marker and the endpoint matches on it, so a project can tell a
 * server-fn POST from a form action POST in a log.
 */
export async function __oxide_call_fn(id: string, args: unknown[]): Promise<unknown> {
  const r = await fetch(`/_oxide/fn/${encodeURIComponent(id)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/oxide-fn' },
    body: JSON.stringify({ args }),
  })
  // 404 → the id is not registered (a stale client bundle, or a fn that
  // was tree-shaken away). Any non-2xx carries the same body shape so the
  // caller gets a useful message instead of a bare fetch failure.
  const text = await r.text()
  let parsed: FnOk | FnErr
  try {
    parsed = JSON.parse(text) as FnOk | FnErr
  } catch {
    throw new Error(`server fn ${id} returned ${r.status} with a non-JSON body: ${text.slice(0, 120)}`)
  }
  if (parsed.ok) {
    return parsed.data
  }
  const e = new Error(parsed.message)
  ;(e as Error & { code?: string }).code = parsed.code
  throw e
}
