// Home page renders a `<Island>` for the Counter component. `<Island>`
// emits the exact `<ox-island>` markup the client boot script scans for;
// the `chunk` prop is a logical island id, resolved at build time through
// `config.framework.islands`.
import { Island } from '@oxide-fw/react'
// Registers the `greet` server function (§9.4). Its import is a no-op in
// the browser — the compiler replaces the module's export with a stub.
import { greet } from '../../greeting.server.ts'
// Referenced so the tree-shaker keeps the stub; a real app would call it.
const greetingFnId = greet.id
import Counter from '../../islands/counter.tsx'

export default function Page() {
  return (
    <main data-greeting-fn={greetingFnId}>
      <h1>Home</h1>
      <Island component="Counter" chunk="Counter" props={{ initial: 3 }}>
        <Counter initial={3} />
      </Island>
    </main>
  )
}
