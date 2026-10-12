import { defineLoad, error } from 'oxide/server'

export const load = defineLoad(async ({ params }) => {
  if (!params.slug) error(404)
  return { slug: params.slug }
})
