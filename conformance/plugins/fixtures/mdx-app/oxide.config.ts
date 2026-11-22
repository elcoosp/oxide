import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as mdx } from '@mdx-js/rollup'

// V2-E05 fixture: wires @mdx-js/rollup into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof mdx === 'function' ? { name: '@mdx-js/rollup', kind: 'js', path: '@mdx-js/rollup', options: {} } : { name: '@mdx-js/rollup', kind: 'js', path: '@mdx-js/rollup' }],
  adapter: 'binary',
})
