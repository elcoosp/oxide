import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as pwa } from 'vite-plugin-pwa'

// V2-E05 fixture: wires vite-plugin-pwa into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof pwa === 'function' ? { name: 'vite-plugin-pwa', kind: 'js', path: 'vite-plugin-pwa', options: {} } : { name: 'vite-plugin-pwa', kind: 'js', path: 'vite-plugin-pwa' }],
  adapter: 'binary',
})
