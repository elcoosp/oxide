import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (solid)' }
})
