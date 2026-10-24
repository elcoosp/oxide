import { defineLoad } from '@oxide-fw/oxide/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (svelte)' }
})
