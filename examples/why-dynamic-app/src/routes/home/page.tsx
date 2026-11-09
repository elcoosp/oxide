export default function Home() {
  return (
    <main>
      <h1>why-dynamic example</h1>
      <p>
        This route has no <code>page.server.ts</code> loader, so it reads no
        request state and is <strong>static</strong> — L0-cacheable on the
        second request.
      </p>
      <ul>
        <li><a href="/dash">/dash — cookie read (dynamic)</a></li>
        <li><a href="/search">/search — searchParams read (dynamic)</a></li>
        <li><a href="/feed">/feed — uncached fetch (static mode, why-reason)</a></li>
      </ul>
    </main>
  )
}
