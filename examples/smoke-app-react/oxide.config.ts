import react from '@oxide-fw/react'
import { defineConfig } from '@oxide-fw/core'

// §14.1: this config declares *where* routes live and which adapter to use.
// Route components are discovered from `src/routes` at build time and
// resolved at render time from the generated component surface (§6.4) — no
// hand-imported page/layout/error map belongs here.

export default defineConfig({
  // §16.3: origin used by the emitted `sitemap.xml` and `robots.txt`.
  siteUrl: 'https://smoke.example.com',
  framework: react({
    ssr: 'real',
    // §10.3 islands registry: component name → source file. The CLI
    // emits a synthetic `__boot__` client entry that imports each of
    // these and registers them; `<Island chunk="X">` on a page is a
    // logical id that resolves to `X` in this map.
    islands: {
      Counter: 'src/islands/counter.tsx',
    },
  }),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',
  modules: [],
  // S-09: the `/_oxide/og` rasterization endpoint is opt-in because it is
  // CPU-bound and unauthenticated. The Playwright `og.spec.ts` suite asserts
  // it end-to-end, so this fixture opts in.
  og: { enabled: true },
  // §V2-F12: the brownout mode is on for the fixture so the e2e pin can
  // flip the governor's level and pin the enforcement stage + the
  // readiness flip on the wire. At `Normal` the mode is inert (one
  // relaxed atomic per request), so the other smoke suites are unchanged.
  brownout: { enabled: true },
  // §V2-F15: the ox-replay fixture's loader fetches the app's own health
  // endpoint through the SSRF-guarded op path; loopback is a blocked
  // range by default, so the fixture allow-lists it (dev fixture only).
  security: { ssrfAllowList: ['127.0.0.1'] },
  // §V2-F15: the ox-replay fixture records every request into
  // `.oxide/records/` (the e2e pin replays one of them). The other smoke
  // suites don't read the records dir; the e2e pin cleans it before and
  // after its own run.
  record: 'all',
  // §16.3 / §16 / K3: a web app manifest makes `oxide build` emit
  // `manifest.webmanifest` and a service worker, and the boot entry then
  // registers it. The Playwright `sw.spec.ts` suite is what asserts the
  // precache contains the offline fallback and that an offline navigation
  // serves it — the browser-level test the SW work was missing.
  manifest: {
    name: 'Oxide Smoke',
    shortName: 'Smoke',
    startUrl: '/',
    display: 'standalone',
  },
})
