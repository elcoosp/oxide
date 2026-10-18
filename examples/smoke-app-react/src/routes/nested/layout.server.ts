import { defineLoad } from 'oxide/server'

export const load = defineLoad(async () => {
  return { org: 'acme-inc' }
})
