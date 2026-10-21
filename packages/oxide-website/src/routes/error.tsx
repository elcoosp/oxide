// src/routes/error.tsx — §10.2 step 5: the nearest error.tsx renders when a
// page throws during SSR.
import { site } from '../lib/site.ts'

export default function ErrorPage() {
  return (
    <main className="error-page container">
      <div className="code">500 · SSR ERROR</div>
      <h1>Something threw during render</h1>
      <p>
        The nearest error boundary caught it. Check the server logs — request waterfalls and
        stage spans are on Server-Timing, in the dev inspector, and via the MCP server.
      </p>
      <a className="btn btn-ghost" href={site.repo} rel="noopener">
        GitHub ↗
      </a>
    </main>
  )
}
