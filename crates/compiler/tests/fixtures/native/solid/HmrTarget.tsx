// V2-C02 fixture (the smoke-component shape): a static multi-child tree
// with a delegated event handler and a single dynamic text hole — the
// `hmr-target.tsx` shape `examples/smoke-app-solid` ships.
import { createSignal } from 'solid-js';

export default function HmrTarget() {
  const [count, setCount] = createSignal(0);
  return (
    <section>
      <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
      <button data-hmr-counter="" type="button" onClick={() => setCount(count() + 1)}>
        {count()}
      </button>
    </section>
  );
}
