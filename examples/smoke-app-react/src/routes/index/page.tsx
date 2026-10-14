// Home page renders a `<Island>` for the Counter component. `<Island>`
// emits the exact `<ox-island>` markup the client boot script scans for;
// the `chunk` prop is a logical island id, resolved at build time through
// `config.framework.islands`.
import { Island } from '@oxide/react'
import Counter from '../../islands/counter.tsx'

export default function Page() {
  return (
    <main>
      <h1>Home</h1>
      <Island component="Counter" chunk="Counter" props={{ initial: 3 }}>
        <Counter initial={3} />
      </Island>
    </main>
  )
}
