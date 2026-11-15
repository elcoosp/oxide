// V2-C02 fixture (the spec's spike shape): a signal, an event handler and
// a `<Show>` — the smallest source that exercises every runtime helper the
// native Solid backend emits on the DOM target (template, insert, delegated
// events, createComponent with getter props) plus the TS strip the worker
// runs (the typed `props`).
import { createSignal, Show } from 'solid-js';

export default function Counter(props: { start: number }) {
  const [count, setCount] = createSignal(props.start);
  const inc = () => setCount(count() + 1);
  return (
    <section class="counter">
      <button type="button" onClick={inc}>
        Clicked {count()} times
      </button>
      <Show when={count() > 0}>
        <p>over zero</p>
      </Show>
    </section>
  );
}
