// §9.3 'use cache' fixture. The compiler rewrites this module's export to
// `__oxide_cache(fn, "<module>", "getPost", { tags: [...], life: {...} })`.
import { cacheTag, cacheLife } from '@oxide-fw/core/server'

export const getPost = async (slug: string) => {
  'use cache'
  cacheTag('posts:' + slug)
  cacheLife({ ttl: 60, swr: 300 })
  return { slug, title: `Post ${slug}`, body: 'cached' }
}
