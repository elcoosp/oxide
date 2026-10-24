// §9.4 server function fixture. `serverFn(fn)` compiles to a stub in the
// client bundle that POSTs to `/_oxide/fn/<id>`; the SSR bundle keeps the
// body and appends `__oxide_register_fn("<id>", greet)` so the worker's
// registry has it.
import { serverFn } from '@oxide-fw/oxide/server'

export const greet = serverFn(async (name: string) => {
  return { message: `Hello ${name} from a server fn` }
})
