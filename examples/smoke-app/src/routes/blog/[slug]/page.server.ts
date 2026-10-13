// The blog loader derives a title and body from the URL slug. The page
// renders `props.data.title` — so a real `oxide render`/`oxide dev` run
// must execute this loader for the output to have content.
import { defineLoad, error } from 'oxide/server'

export const load = defineLoad(async ({ params }) => {
  const slug = params.slug
  if (!slug) error(404, 'slug required')
  return {
    slug,
    title: `Hello ${slug}`,
    body: `This is the post for "${slug}".`,
  }
})
