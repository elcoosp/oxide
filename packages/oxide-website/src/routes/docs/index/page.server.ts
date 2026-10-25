// src/routes/docs/index/page.server.ts — /docs renders the getting-started
// page (same content as /docs/getting-started; the radix tree keeps both).
import { defineLoad, defineMeta, error } from '@oxide-fw/core/server'
import { readDoc, readDocMeta } from '../../../lib/md.server.ts'

export const config = { render: 'isr', revalidate: 3600, tags: ['docs'] }

export const load = defineLoad<{ doc: NonNullable<Awaited<ReturnType<typeof readDoc>>> }>(async () => {
  const doc = await readDoc('getting-started')
  if (!doc) error(500, 'content/docs/getting-started.md missing')
  return { doc, path: 'getting-started' }
})

export const meta = defineMeta(({ data }) => {
  const meta = readDocMeta(data)
  return {
    title: `${meta.title} — Oxide`,
    description: meta.description,
  }
})
