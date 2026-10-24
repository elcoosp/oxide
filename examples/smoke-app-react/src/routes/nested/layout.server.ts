import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  return { org: 'acme-inc' }
})
