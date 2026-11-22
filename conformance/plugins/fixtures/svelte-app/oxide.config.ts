import { defineConfig } from '@oxide-fw/core'
import { adapter as svelteAdapter } from '@oxide-fw/svelte'
import { default as svelte } from '@sveltejs/vite-plugin-svelte'

// V2-E05 fixture: wires @sveltejs/vite-plugin-svelte into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: svelteAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof svelte === 'function' ? { name: '@sveltejs/vite-plugin-svelte', kind: 'js', path: '@sveltejs/vite-plugin-svelte', options: {} } : { name: '@sveltejs/vite-plugin-svelte', kind: 'js', path: '@sveltejs/vite-plugin-svelte' }],
  adapter: 'binary',
})
