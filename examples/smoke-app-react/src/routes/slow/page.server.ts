// §8.4: a slow loader so the client navigation's loading boundary is visible.
// The delay is short (250ms) so the test does not wait long, but long enough
// to guarantee the loading overlay appears before the fetch resolves.
import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  await new Promise((r) => setTimeout(r, 250))
  return { message: 'finally loaded' }
})
