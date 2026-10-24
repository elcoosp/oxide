// §9.3: an **actions-only** server module — no `load`. The route has no
// loader, so its data is empty and the page renders with `data.form` from
// the action.
//
// This shape used to be rejected: the worker treated any `page.server.ts`
// as the route's data module and errored unless it exported `load`, which
// made an actions-only route unusable — the form POST could not re-render
// the page it belonged to. The worker now accepts `actions`-or-`config`-
// only modules and yields empty data.
import { defineAction } from '@oxide-fw/core/server'

export const actions = {
  comment: defineAction({
    run: async ({ input }: { input: Record<string, string> }) => {
      const text = String(input?.text ?? '').trim()
      return { ok: true, received: text }
    },
  }),
  setcookie: defineAction({
    run: async () => new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: {
        'content-type': 'application/json',
        'set-cookie': 'oxide_b6=testvalue; Path=/; HttpOnly',
      },
    }),
  }),
}
