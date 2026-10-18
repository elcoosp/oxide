import { defineLoad } from 'oxide/server'

export const load = defineLoad(async ({ parent }: { parent: () => Promise<{ org?: string }> }) => {
  const p = await parent()
  return { parentOrg: p?.org ?? 'MISSING' }
})
