// The blog loader derives a title and body from the URL slug. The page
// renders `props.data.title` — so a real `oxide render`/`oxide dev`/build
// run must execute this loader for the output to have content.
//
// `generateParams` returns a runtime-built array (`.map`), which forces
// the compiler's static extractor into the Runtime case. `oxide build`
// then evaluates it through the worker to enumerate the prerender paths.
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

// The `.map` call defeats static extraction; the worker evaluates it.
export const generateParams = () =>
  ['hello-prod', 'second-post'].map((slug) => ({ slug }))
