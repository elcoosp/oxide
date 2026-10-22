// src/routes/blog/index/page.server.ts
import { defineLoad } from 'oxide/server'
import { listPosts } from '../../../lib/md.server.ts'

export const load = defineLoad(async () => {
  return { posts: await listPosts() }
})
