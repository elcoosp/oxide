// src/routes/index/page.server.ts — home page loader. Everything it returns
// is ODF-encoded and cacheable; deferred values would stream here (see the
// repo README for the defer() showcase) — this page keeps it simple and
// fully cached.
import { defineLoad } from 'oxide/server'
import { listPosts } from '../../lib/md.server.ts'

export const load = defineLoad(async () => {
  return {
    latest: listPosts().slice(0, 3),
  }
})
