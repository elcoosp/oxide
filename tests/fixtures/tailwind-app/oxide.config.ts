import { defineConfig } from '@oxide-fw/core'
import { tailwind } from '@oxide-fw/tailwind'
import react from '@oxide-fw/react'

export default defineConfig({
  plugins: [tailwind()],
  framework: react(),
  routes: { dir: 'src/routes' },
})
