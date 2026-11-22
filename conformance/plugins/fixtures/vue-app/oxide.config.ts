import { defineConfig } from '@oxide-fw/core'
import { adapter as vueAdapter } from '@oxide-fw/vue'
import { default as vue } from '@vitejs/plugin-vue'

// V2-E05 fixture: wires @vitejs/plugin-vue into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: vueAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof vue === 'function' ? { name: '@vitejs/plugin-vue', kind: 'js', path: '@vitejs/plugin-vue', options: {} } : { name: '@vitejs/plugin-vue', kind: 'js', path: '@vitejs/plugin-vue' }],
  adapter: 'binary',
})
