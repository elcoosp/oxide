// §8.4: a slow loader so the client navigation's loading boundary is visible.
// The delay is short (250ms) so the test does not wait long, but long enough
// to guarantee the loading overlay appears before the fetch resolves.
import { defineLoad } from '@oxide-fw/core/server'

// §V2-F12: the route's overload priority — a slow dynamic route is the
// canonical brownout shed target, so the fixture declares `low` (the
// enforcement stage sheds it with 503 + `x-oxide-brownout: 2` at level
// `Shed`, before any loader work).
export const config = {
  priority: 'low',
}

export const load = defineLoad(async () => {
  await new Promise((r) => setTimeout(r, 250))
  return { message: 'finally loaded' }
})
