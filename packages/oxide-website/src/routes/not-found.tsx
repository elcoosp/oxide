// src/routes/not-found.tsx — §10.2 step 5: unmatched URLs render this page
// with a 404 status.
import { site } from '../lib/site.ts'

export default function NotFound() {
  return (
    <main className="error-page container">
      <div className="code">404 · NOT FOUND</div>
      <h1>No route matched that URL</h1>
      <p>
        The rkyv route table is memory-mapped at boot and this path is not in it — which is itself
        a small demonstration of the routing fast path: the miss was evaluated natively in Rust,
        without entering a JavaScript runtime.
      </p>
      <a className="btn btn-primary" href="/">
        Back home
      </a>{' '}
      <a className="btn btn-ghost" href={site.repo} rel="noopener">
        GitHub ↗
      </a>
    </main>
  )
}
