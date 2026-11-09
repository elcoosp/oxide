// V2-F16 fixture route: the fetch is NOT inside a `'use cache'` function,
// so `infer_reasons` reports an `uncachedFetch` reason while the render
// mode stays `static` (a fetch reads no request state — it is a
// cacheability smell, not a mode trigger).
export async function load() {
  const res = await fetch('https://example.com/api/feed')
  const items = await res.json()
  return { items }
}
