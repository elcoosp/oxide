// §7.1 / B2: a deterministic deferred-boundary fixture for the Playwright
// suite. The fast slot settles first and the slow one ~800ms later, so a
// streamed response must flush two resolution scripts at distinct times —
// not batch them after the slowest promise.
import { defineLoad } from '@oxide-fw/oxide/server'

export const config = { render: 'dynamic' }

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

export const load = defineLoad(async ({ defer }) => ({
  fast: defer(sleep(80).then(() => ({ label: 'fast' }))),
  slow: defer(sleep(900).then(() => ({ label: 'slow' }))),
}))
