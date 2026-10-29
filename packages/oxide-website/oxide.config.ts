// oxide.config.ts — the oxide.dev website, built with Oxide itself.
//
// §14.1 / §6.4: this config declares *where* routes live and which
// adapter to use. Route components are discovered from `src/routes` at
// build time and resolved at render time from the generated component
// surface (the SSR adapter entry). No config-side imports, no
// `componentFor` closure — that was the pre-§6.4 shape, migrated when
// the build learned to own the route → component mapping.
import react from '@oxide-fw/react'
import { tailwind } from '@oxide-fw/tailwind'
import { defineConfig } from '@oxide-fw/core'

export default defineConfig({
  // §6.6 / D5: Tailwind v4 native plugin. Precomputes CSS from `src/styles/tailwind.css`
  // using Tailwind's own Rust scanner + compiler.
  plugins: [tailwind()],

  // §16.3: origin used by the emitted sitemap.xml and robots.txt.
  siteUrl: 'https://oxide.dev',

  framework: react({
    ssr: 'real',
    // §10.3 islands registry: the CLI emits a synthetic `__boot__` client
    // entry that imports each of these and registers them; `<Island
    // chunk="Stats">` on a page resolves through this map.
    islands: {
      Stats: 'src/islands/stats.tsx',
      CopyInstall: 'src/islands/copy-install.tsx',
      AdapterTabs: 'src/islands/adapter-tabs.tsx',
      RevealBootstrap: 'src/islands/reveal.tsx',
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
  // create-oxide scaffolder emits the same shape now; a flat
  // `l0MaxSize` is rejected by `CacheConfig`'s `deny_unknown_fields`.
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
  // markdown sources for the blog and the docs. Loaders call
  // `getCollection('blog')` / `getEntry('blog', slug)` (§16.5, wired) which
  // reads the emitted `dist/content/*.entries.json`.
  content: {
    collections: [
      { name: 'blog', dir: 'content/blog', indexedFields: ['title', 'date', 'tags'] },
      { name: 'docs', dir: 'content/docs', indexedFields: ['title', 'order'] },
    ],
  },
})
