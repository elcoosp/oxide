// §7.1 / B2: a deterministic deferred-boundary fixture. Each `defer()` in
// the loader arrives as `{__deferred: <id>}` server-side, is wired into a
// `DeferredClient` client-side, and resolves when the matching
// `<script type="application/oxide-deferred" data-id="<id>">` is parsed.
// The two `<span data-deferred-value>` markers below are the DOM landing
// pad: the client glue (`attachDeferGlue` → `resolveSlot` → `updateMarker`)
// writes each resolved slot's `{label: string}` value into its marker, so
// the Playwright spec can assert that a value that arrived over the wire
// is visible on the page.
export default function Deferred() {
  return (
    <main data-deferred-page="">
      <h1>Deferred</h1>
      <span data-deferred-value="1">loading</span>
      <span data-deferred-value="2">loading</span>
    </main>
  )
}
