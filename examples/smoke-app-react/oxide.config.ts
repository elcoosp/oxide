import react from '@oxide/react'
import { defineConfig } from 'oxide'

// The pages are .tsx; the worker's tsx-loader.mjs applies the JSX transform
// at import time, so we can reference them here exactly as a user would.
// @ts-expect-error - .tsx module resolved at runtime by the worker's loader
import Home from './src/routes/index/page.tsx'
// @ts-expect-error
import Post from './src/routes/blog/[slug]/page.tsx'

// The home page renders a `<ox-island>` for the Counter component
// (see src/islands/counter.tsx). The Playwright suite in tests/playwright/
// asserts the SSR'd island markup end-to-end.

/** Route id → React component. Route ids are the route *directories*
 *  (no trailing `/page`), matching what the pipeline normalizes to before
 *  handing the id to `componentFor`. */
const components: Record<string, unknown> = {
  'routes/index': Home,
  'routes/blog/[slug]': Post,
}

export default defineConfig({
  // §16.3: origin used by the emitted `sitemap.xml` and `robots.txt`.
  siteUrl: 'https://smoke.example.com',
  framework: react({
    ssr: 'real',
    componentFor: (route: string) => components[route],
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
})
