// §10.2 step 5: a `not-found.tsx` sibling renders on any unmatched URL, with
// a 404 status. The router's `not_found` route kind resolves this file; the
// CLI substitutes it for the page id when the URL matches nothing.
export default function NotFound() {
  return (
    <main data-not-found="">
      <h1>404</h1>
      <p>This page does not exist.</p>
    </main>
  )
}
