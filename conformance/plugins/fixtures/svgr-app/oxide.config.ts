import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as svgr } from 'vite-plugin-svgr'

// V2-E05 fixture: wires vite-plugin-svgr into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof svgr === 'function' ? { name: 'vite-plugin-svgr', kind: 'js', path: 'vite-plugin-svgr', options: {} } : { name: 'vite-plugin-svgr', kind: 'js', path: 'vite-plugin-svgr' }],
  adapter: 'binary',
})
