// src/routes/blog/[slug]/page.server.ts — the blog post loader.
//
// ISR + cache tags: the rendered post fills the L0/L1 caches tagged
// `blog`, so publishing a new post can invalidate precisely. Markdown is
// read and rendered server-side (see src/lib/md.server.ts); `meta` merges
// into the neutral head model (§16.3).
import { defineLoad, defineMeta, error } from '@oxide-fw/oxide/server'
import { readPost } from '../../../lib/md.server.ts'

export const config = { render: 'isr', revalidate: 3600, tags: ['blog'] }

export const load = defineLoad(async ({ params }) => {
  const slug = String(params?.slug ?? '')
  const post = await readPost(slug)
  if (!post) error(404, `no blog post for "${slug}"`)
  return { post }
})

export const meta = defineMeta(({ data }) => ({
  title: `${data?.post?.meta?.title ?? 'Blog'} — Oxide`,
  description: data?.post?.meta?.description ?? '',
}))

// Worker-evaluated at build time to enumerate prerender slugs. The `.map`
// call shape keeps this out of static extraction on purpose.
export function generateParams() {
  return [{ slug: 'introducing-oxide' }, { slug: 'one-wire-format-two-languages' }, { slug: 'auditing-our-own-framework-91-of-100' }].map(
    (x) => x,
  )
}
