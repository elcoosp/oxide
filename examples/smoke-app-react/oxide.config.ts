import react from '@oxide/react'
import { defineConfig } from 'oxide'

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
})
