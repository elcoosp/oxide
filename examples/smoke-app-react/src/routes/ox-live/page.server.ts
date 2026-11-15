// §V2-F05/F06 fixture: a live route. The loader's module-level counter
// makes every rerun observable; the `bump` action calls
// `revalidate.path('/ox-live')`, which the dispatch applies to the cache
// AND fans out to the live hub — the subscribers re-run their loaders and
// push the new snapshot to their open tabs (the `path:` tag the endpoint
// seeds each subscriber with matches this event).
//
// `config.live` is the scanner's opt-in (`live: true` rides the same
// best-effort config read as the render mode — see crates/router scan).
import { defineAction } from '@oxide-fw/core/server'

let n = 0

export const config = { live: true }

export const load = async () => {
  n += 1
  return { n }
}

export const actions = {
  bump: defineAction({
    run: async ({ revalidate }) => {
      revalidate.path('/ox-live')
      return { ok: true }
    },
  }),
}
