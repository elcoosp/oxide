// §8.4 loading boundary: rendered into an overlay by the client router while
// a navigation's `/__data` fetch is in flight. A real loading.tsx can be any
// component (a skeleton, a spinner); this one carries a data attribute so
// the end-to-end test can find it.
export default function Loading() {
  return (
    <div data-loading-slow="">
      <p>Loading slow page…</p>
    </div>
  )
}
