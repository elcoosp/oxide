import { defineConfig } from '@oxide-fw/core'
import { adapter as solidAdapter } from '@oxide-fw/solid'
import { default as solid } from 'vite-plugin-solid'

// V2-E05 fixture: wires vite-plugin-solid into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: solidAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof solid === 'function' ? { name: 'vite-plugin-solid', kind: 'js', path: 'vite-plugin-solid', options: {} } : { name: 'vite-plugin-solid', kind: 'js', path: 'vite-plugin-solid' }],
  adapter: 'binary',
})
