import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as react } from '@vitejs/plugin-react'

// V2-E05 fixture: wires @vitejs/plugin-react into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof react === 'function' ? { name: '@vitejs/plugin-react', kind: 'js', path: '@vitejs/plugin-react', options: {} } : { name: '@vitejs/plugin-react', kind: 'js', path: '@vitejs/plugin-react' }],
  adapter: 'binary',
})
