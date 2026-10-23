// §9.3: a form-first action. `POST ?/comment` runs it with the submitted
// form fields; the page re-renders with the result in `data.form`. Works
// with JavaScript disabled — the §22 requirement `no-js.spec.ts` asserts.
//
// The trivial `load` is not decorative: the worker treats any
// `page.server.ts` as the route's data module and errors if it exports
// `actions` without `load`. An actions-only module arguably should be
// valid, but that is a change to the worker's contract, not to this
// fixture.
import { defineAction, defineLoad } from 'oxide/server'

export const load = defineLoad(async () => ({}))

export const actions = {
  comment: defineAction({
    run: async ({ input }: { input: Record<string, string> }) => {
      const text = String(input?.text ?? '').trim()
      return { ok: true, received: text }
    },
  }),
}
