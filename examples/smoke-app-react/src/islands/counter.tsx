// Counter island — the reference fixture for the Playwright suite.
//
// `<Island>` wraps this in `<ox-island data-ox-island="Counter" ...>`. The
// page writes `<Island component="Counter" chunk="Counter" props={{ initial }}>`.
// At build time the CLI generates a `__boot__` client entry that imports
// this file and registers it under the key `"Counter"`, then injects
// `<script type="module" src="/client/<hashed-boot>">` into every
// prerendered page. In the browser, the boot module scans
// `<ox-island[data-ox-chunk]>`, decodes props, and calls the adapter's
// `hydrateIsland` to bind the click handler.
import { useState } from 'react'

export default function Counter({ initial = 0 }: { initial?: number }) {
  const [n, setN] = useState(initial)
  return (
    <button type="button" onClick={() => setN(n + 1)}>
      <span>{n}</span>
    </button>
  )
}
