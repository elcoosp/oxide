// src/routes/docs/[page]/page.server.ts — renders any doc page by slug.
import { defineLoad, defineMeta, error } from '@oxide-fw/core/server'
import { readDoc, readDocMeta } from '../../../lib/md.server.ts'

export const config = { render: 'isr', revalidate: 3600, tags: ['docs'] }

export const load = defineLoad<{ doc: NonNullable<Awaited<ReturnType<typeof readDoc>>> }>(async ({ params }) => {
  const slug = String(params?.page ?? '')
  const doc = await readDoc(slug)
  if (!doc) error(404, `no doc page for "${slug}"`)
  return { doc }
})

export const meta = defineMeta(({ data }) => {
  const meta = readDocMeta(data)
  return {
    title: `${meta.title} — Oxide`,
    description: meta.description,
  }
})

// Worker-evaluated at build time to prerender every doc page.
export function generateParams() {
  return ['architecture', 'wire-format-odf', 'adapters', 'caching', 'cli-reference'].map((p) => ({ page: p }))
}
