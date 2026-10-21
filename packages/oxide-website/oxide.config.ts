// oxide.config.ts — the oxide.dev website, built with Oxide itself.
//
// This config deliberately follows the smoke-app shape (config-side page
// imports + componentFor) because that is what real React SSR requires
// today; see docs/deviations.md discussion of the routes authoring model
// (audit finding P0-10). When build-time server-component collection ships,
// this file collapses to `framework: react({ islands })` + `routes.path`.
import react from '@oxide/react'
import { defineConfig } from 'oxide'

// @ts-expect-error - .tsx module resolved at runtime by the worker's loader
import RootLayout from './src/routes/layout.tsx'
// @ts-expect-error
import Home from './src/routes/index/page.tsx'
// @ts-expect-error
import NotFound from './src/routes/not-found.tsx'
// @ts-expect-error
import ErrorPage from './src/routes/error.tsx'
// @ts-expect-error
import BlogIndex from './src/routes/blog/index/page.tsx'
// @ts-expect-error
import Post from './src/routes/blog/[slug]/page.tsx'
// @ts-expect-error
import DocsLayout from './src/routes/docs/layout.tsx'
// @ts-expect-error
import DocsIndex from './src/routes/docs/index/page.tsx'
// @ts-expect-error
import DocPage from './src/routes/docs/[page]/page.tsx'

/** Route id → component. Route ids are the route *directories* (no
 *  trailing `/page`), matching what the pipeline normalizes to before
 *  handing the id to `componentFor`. Layouts, not-found and error pages
 *  resolve through this same map (§8.2, §10.2 step 5). */
const components: Record<string, unknown> = {
  'routes/layout': RootLayout,
  'routes/index': Home,
  'routes/not-found': NotFound,
  'routes/error': ErrorPage,
  'routes/blog/index': BlogIndex,
  'routes/blog/[slug]': Post,
  'routes/docs/layout': DocsLayout,
  'routes/docs/index': DocsIndex,
  'routes/docs/[page]': DocPage,
}

export default defineConfig({
  // §16.3: origin used by the emitted sitemap.xml and robots.txt.
  siteUrl: 'https://oxide.dev',

  framework: react({
    ssr: 'real',
    componentFor: (route: string) => components[route],
    // §10.3 islands registry: the CLI emits a synthetic `__boot__` client
    // entry that imports each of these and registers them; `<Island
    // chunk="Stats">` on a page resolves through this map.
    islands: {
      Stats: 'src/islands/stats.tsx',
    },
  }),

  routes: { dir: 'src/routes' },

  server: {
    port: 3000,
    compression: ['zstd', 'br'],
    // HTTP/3 over QUIC with automatic Alt-Svc advertisement requires a TLS
    // listener. Flip to true when the site is behind real TLS (prod), keep
    // false for plain local dev.
    http3: false,
  },

  render: { default: 'auto' },

  // NOTE: the nested `l0.maxSize` key is the typed surface (§14.1). The
  // create-oxide scaffolder still emits a flat `l0MaxSize`, which
  // CacheConfig's deny_unknown_fields rejects — don't copy it from there.
  cache: { handler: { type: 'disk' }, l0: { maxSize: '512mb' } },

  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },

  // Single static binary deploy (Full capability row: L0 cache, ISR,
  // native rules, snapshots, TLS). See README for the Docker variant.
  adapter: 'binary',

  modules: [],

  // S-09: opt-in OG rasterization endpoint — we use /_oxide/og to mint the
  // social cards shared on X and in newsletter pitches (see the launch kit).
  og: { enabled: true },

  // §16.5 content collections: build-time scan/validation/indexing of the
  // markdown sources for the blog and the docs. The loaders read the same
  // files directly (runtime collection queries are still being wired), but
  // the build already emits the typed content.d.ts + search indexes.
  content: {
    collections: [
      { name: 'blog', dir: 'content/blog', indexedFields: ['title', 'date', 'tags'] },
      { name: 'docs', dir: 'content/docs', indexedFields: ['title', 'order'] },
    ],
  },
})
