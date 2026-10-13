import react from '@oxide/react'
import { defineConfig } from 'oxide'

// The pages are .tsx; the worker's tsx-loader.mjs applies the JSX transform
// at import time, so we can reference them here exactly as a user would.
// @ts-expect-error - .tsx module resolved at runtime by the worker's loader
import Home from './src/routes/index/page.tsx'
// @ts-expect-error
import Post from './src/routes/blog/[slug]/page.tsx'

/** Route id → React component. Route ids are the route *directories*
 *  (no trailing `/page`), matching what the pipeline normalizes to before
 *  handing the id to `componentFor`. */
const components: Record<string, unknown> = {
  'routes/index': Home,
  'routes/blog/[slug]': Post,
}

export default defineConfig({
  framework: react({
    ssr: 'real',
    componentFor: (route: string) => components[route],
  }),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',
  modules: [],
})
