import { createSignal } from 'solid-js'

// HMR fixture: the e2e suite rewrites the marker text in place and watches
// the DOM update without a navigation. The counter pins the remount
// semantics of component-level HMR (state resets, DOM updates in place).
export default function HmrTarget() {
  const [count, setCount] = createSignal(0)
  return (
    <section>
      <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
      <button data-hmr-counter="" type="button" onClick={() => setCount(count() + 1)}>
        {count()}
      </button>
    </section>
  )
}
