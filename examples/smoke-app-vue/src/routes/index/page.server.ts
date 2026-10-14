import { defineLoad } from 'oxide/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (vue)' }
})
