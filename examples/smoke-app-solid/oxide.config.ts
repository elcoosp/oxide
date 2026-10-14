import solid from '@oxide/solid'
import { defineConfig } from 'oxide'

// Solid's JSX compiles through `solid-jsx-oxc` — a Rust/OXC-native compiler
// with NAPI bindings, not babel. The CLI detects `framework: solid(...)`
// and routes `.tsx` through the tooling pool (`tooling:solid-compile`),
// which calls `solid-jsx-oxc.transform({ generate: 'ssr' })`. The output
// is `ssr`-template-literal JS importing from `solid-js/web`, exactly what
// `@solidjs/web`'s `renderToString` consumes.
// @ts-expect-error - .tsx resolved at runtime by the worker's tsx-loader
import Home from './src/routes/index/page.tsx'

const components: Record<string, unknown> = {
  'routes/index': Home,
}

export default defineConfig({
  framework: solid({
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
