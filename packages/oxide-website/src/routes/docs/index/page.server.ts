// src/routes/docs/index/page.server.ts — /docs renders the getting-started
// page (same content as /docs/getting-started; the radix tree keeps both).
import { defineLoad, defineMeta, error } from '@oxide-fw/oxide/server'
import { readDoc } from '../../../lib/md.server.ts'

export const config = { render: 'isr', revalidate: 3600, tags: ['docs'] }

export const load = defineLoad(async () => {
  const doc = await readDoc('getting-started')
  if (!doc) error(500, 'content/docs/getting-started.md missing')
  return { doc, path: 'getting-started' }
})

export const meta = defineMeta(({ data }) => ({
  title: `${data?.doc?.meta?.title ?? 'Docs'} — Oxide`,
  description: data?.doc?.meta?.description ?? '',
}))
