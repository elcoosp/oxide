import svelte from '@oxide/svelte'
import { defineConfig } from 'oxide'

// @ts-expect-error - .svelte module resolved at runtime by the worker's loader
import Home from './src/routes/index/page.svelte'

const components: Record<string, unknown> = {
  'routes/index': Home,
}

export default defineConfig({
  framework: svelte({
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
