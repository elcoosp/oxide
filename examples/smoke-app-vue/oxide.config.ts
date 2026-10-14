import vue from '@oxide/vue'
import { defineConfig } from 'oxide'

// The page is .vue; the worker's tsx-loader.mjs compiles it at load time
// through @vue/compiler-sfc, so the config can import it directly.
// @ts-expect-error - .vue module resolved at runtime by the worker's loader
import Home from './src/routes/index/page.vue'

const components: Record<string, unknown> = {
  'routes/index': Home,
}

export default defineConfig({
  framework: vue({
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
