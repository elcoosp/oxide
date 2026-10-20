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
// §8.2: layout components nest the page. The adapter resolves each layout id
// (from the router's layout_chain, e.g. `routes/layout`) via this same
// `componentFor` lookup, then wraps the page: RootLayout(Page).
// @ts-expect-error - .tsx module resolved at runtime by the worker's loader
import RootLayout from './src/routes/layout.tsx'
// §10.2 step 5: unmatched URLs render this page with a 404 status.
// @ts-expect-error
import NotFound from './src/routes/not-found.tsx'
// §10.2 step 5: nearest error.tsx renders on SSR throw.
// @ts-expect-error
import ErrorPage from './src/routes/error.tsx'
// §10.2 step 5 smoke fixture: a page that throws on purpose.
// @ts-expect-error
import Boom from './src/routes/boom/page.tsx'
// §8.1 middleware.ts smoke fixture: a page whose loader reads `locals`.
// @ts-expect-error
import Greeting from './src/routes/greeting/page.tsx'
// §8.4 loading boundary smoke fixture: a slow page.
// @ts-expect-error
import Slow from './src/routes/slow/page.tsx'
// @ts-expect-error
import Nested from './src/routes/nested/page.tsx'
// @ts-expect-error
import NestedLayout from './src/routes/nested/layout.tsx'

const components: Record<string, unknown> = {
  'routes/index': Home,
  'routes/blog/[slug]': Post,
  'routes/layout': RootLayout,
  'routes/not-found': NotFound,
  'routes/error': ErrorPage,
  'routes/boom': Boom,
  'routes/greeting': Greeting,
  'routes/slow': Slow,
  'routes/nested': Nested,
  'routes/nested/layout': NestedLayout,
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
  // S-09: the `/_oxide/og` rasterization endpoint is opt-in because it is
  // CPU-bound and unauthenticated. The Playwright `og.spec.ts` suite asserts
  // it end-to-end, so this fixture opts in.
  og: { enabled: true },
})
