// The client side of the resumable-stream contract (spec v0.2, §V2-F07).
//
// A response marked `x-oxide-stream: <id>` (the server tees it into the
// stream store while it streams out) survives a dropped connection: the
// first pass pumps the body; a mid-stream drop reattaches
// `GET /__oxide/stream/<id>` with `Range: bytes=<n>-` carrying the exact
// byte count, up to five tries on the spec's backoff ladder (200 ms
// doubling). A `410` means the stream expired — the caller fetches fresh.
//
// Wire contract (server side: `oxide_fw_server::stream_store`):
//   x-oxide-stream: <id>            the response is resumable
//   GET /__oxide/stream/<id>        reattach (a data-plane route: the A04
//     Range: bytes=<n>-             build check applies)
//   206 + content-range: bytes n-*/*    backlog + live tail
//   200 + x-oxide-stream-complete: 1    the stream was fully consumed
//   410                                   expired/overflowed: fetch fresh
//
// Module-safety contract (same as `live.ts` / `build.ts`): import-safe in
// the SSR / config-loader Node environment — no top-level side effects;
// the fetch implementation and the backoff sleep are injectable so the
// Node test runner can pin the behavior without a network.

/** Options for [`resumableFetch`] — injectables for the Node test runner. */
export type ResumableFetchOptions = {
  /** The fetch implementation (default: `globalThis.fetch`). */
  fetch?: typeof fetch
  /**
   * The backoff wait between reattach tries, ms (default: `setTimeout`).
   * The spec ladder is 200 * 2**try for try in 0..5.
   */
  sleep?: (ms: number) => Promise<void>
}

/** The default wait: a plain `setTimeout` promise. */
const defaultSleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms))

/**
 * Fetch a (possibly) resumable response. Behaves exactly like `fetch` for
 * a response without the `x-oxide-stream` header. With it, a body that
 * errors mid-stream is reattached at the byte offset the consumer already
 * saw — `onChunk` receives each chunk exactly once, in order, across the
 * drop. The returned `Response` is the original one (status/headers);
 * callers read the body through `onChunk` (or ignore it).
 *
 * Throws `stream expired` when the server answers the reattach with `410`
 * (the buffer's TTL passed or it overflowed), and `stream lost` when five
 * reattach tries all fail.
 */
export async function resumableFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
  onChunk?: (chunk: Uint8Array) => void,
  opts: ResumableFetchOptions = {},
): Promise<Response> {
  const fetchImpl = opts.fetch ?? fetch
  const sleep = opts.sleep ?? defaultSleep
  const res = await fetchImpl(input, init)
  const id = res.headers.get('x-oxide-stream')
  if (!id || !res.body) return res
  let got = 0
  async function pump(r: Response): Promise<boolean> {
    // A non-ok reattach (a gateway 503) can carry a null body: the reader
    // construction is inside the try so it counts as a failed try instead
    // of escaping.
    try {
      const rd = r.body!.getReader()
      for (;;) {
        const { done, value } = await rd.read()
        if (done) return true
        got += value.length
        onChunk?.(value)
      }
    } catch {
      return false
    }
  }
  let ok = await pump(res)
  for (let tries = 0; !ok && tries < 5; tries++) {
    await sleep(200 * 2 ** tries)
    const r2 = await fetchImpl('/__oxide/stream/' + id, {
      headers: { Range: 'bytes=' + got + '-' },
    })
    if (r2.status === 410) throw new Error('stream expired')
    ok = await pump(r2)
  }
  if (!ok) throw new Error('stream lost')
  return res
}
