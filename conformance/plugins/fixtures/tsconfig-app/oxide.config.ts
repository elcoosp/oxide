import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as tsconfig-paths } from 'vite-tsconfig-paths'

// V2-E05 fixture: wires vite-tsconfig-paths into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof tsconfig-paths === 'function' ? { name: 'vite-tsconfig-paths', kind: 'js', path: 'vite-tsconfig-paths', options: {} } : { name: 'vite-tsconfig-paths', kind: 'js', path: 'vite-tsconfig-paths' }],
  adapter: 'binary',
})
