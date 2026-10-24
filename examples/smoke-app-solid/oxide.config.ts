import solid from '@oxide-fw/solid'
import { defineConfig } from '@oxide-fw/core'

// §14.1: this config declares *where* routes live and which adapter to use.
// Route components are discovered from `src/routes` at build time and
// resolved at render time from the generated component surface (§6.4) — no
// hand-imported page/layout/error map belongs here.

export default defineConfig({
  framework: solid({
    ssr: 'real',
  }),
  routes: { dir: 'src/routes' },
  server: { port: 3000, compression: ['zstd', 'br'], http3: false },
  render: { default: 'auto' },
  build: { target: 'baseline-widely-available', sourcemap: 'hidden' },
  adapter: 'binary',
  modules: [],
})
