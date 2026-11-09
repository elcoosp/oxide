import react from '@oxide-fw/react'
import { defineConfig } from '@oxide-fw/core'

// Same config shape as the smoke apps: this file declares *where* routes
// live and which adapter renders them (§14.1). The why-dynamic walk is a
// build-time AST pass over `src/routes/**/page.server.ts` — it never runs
// this file and never enters a JS runtime.

export default defineConfig({
  siteUrl: 'https://why-dynamic.example.com',
  framework: react({
    ssr: 'real',
  }),
})
