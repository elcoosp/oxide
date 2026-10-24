import { defineLoad } from '@oxide-fw/oxide/server'

export const load = defineLoad(async () => {
  return { org: 'acme-inc' }
})
