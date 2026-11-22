import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as tailwind } from '@tailwindcss/vite'

// V2-E05 fixture: wires @tailwindcss/vite into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof tailwind === 'function' ? { name: '@tailwindcss/vite', kind: 'js', path: '@tailwindcss/vite', options: {} } : { name: '@tailwindcss/vite', kind: 'js', path: '@tailwindcss/vite' }],
  adapter: 'binary',
})
