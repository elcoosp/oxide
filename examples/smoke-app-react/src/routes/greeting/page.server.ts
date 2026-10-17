// Reads the middleware-injected `locals.user` — proof that §8.1 middleware
// locals reach the §9.1 loader event.
import { defineLoad } from 'oxide/server'

export const load = defineLoad(async ({ locals }: { locals?: { user?: string; role?: string } }) => {
  return { greeting: `hello, ${locals?.user ?? 'guest'}`, role: locals?.role ?? 'anon' }
})
