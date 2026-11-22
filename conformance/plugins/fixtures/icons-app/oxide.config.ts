import { defineConfig } from '@oxide-fw/core'
import { adapter as reactAdapter } from '@oxide-fw/react'
import { default as icons } from 'unplugin-icons'

// V2-E05 fixture: wires unplugin-icons into the plugin chain so the
// bridge's load-time unsupported-hook report runs against it.
export default defineConfig({
  framework: reactAdapter(),
  routes: { dir: 'src/routes' },
  plugins: [typeof icons === 'function' ? { name: 'unplugin-icons', kind: 'js', path: 'unplugin-icons', options: {} } : { name: 'unplugin-icons', kind: 'js', path: 'unplugin-icons' }],
  adapter: 'binary',
})
