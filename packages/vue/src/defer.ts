// §7.1: `<Defer>` for `suspense: 'core'` adapters.
//
// Vue's own Suspense is partial (§7.1), so the core supplies the boundary.
// `<Defer>` renders an `<ox-defer data-ox-b>` host during the shell pass and
// registers the resolved content; the adapter streams it as
// `[data-ox-c]` + swapper after the shell.
//
// `rawData` (not `data`) is what carries the placeholder: the adapter hands
// components a pre-resolved `data` so a plain `props.data.x` read gets the
// value, and the same tree unsubstituted as `rawData` so this component can
// see that a boundary was requested.

import { defineComponent, h, type PropType } from 'vue'
import { registerBoundary, deferredIdOf } from '../../oxide/src/server/boundary.ts'

type Slots = Record<string, ((args: { value: unknown }) => unknown) | undefined>

/** Renders the resolved content standalone, after the shell. */
const ResolvedContent = defineComponent({
  name: 'OxideDeferredContent',
  props: {
    value: { type: null as unknown as PropType<unknown>, required: false },
    slots: { type: Object as PropType<Slots>, required: false },
  },
  setup(props) {
    return () => props.slots?.default?.({ value: props.value }) ?? null
  },
})

/**
 * A deferred boundary.
 *
 * - `value` is a placeholder and a resolution exists → emit the placeholder
 *   host and register the content component; the adapter streams it.
 * - `value` is already resolved → render inline, no boundary.
 * - placeholder with no resolution → the fallback stays; the settle failed.
 */
export const Defer = defineComponent({
  name: 'OxideDefer',
  props: {
    /** The placeholder from `props.rawData.<key>`. */
    value: { type: null as unknown as PropType<unknown>, required: false },
    deferred: { type: Object as PropType<Record<string, unknown>>, required: false },
  },
  setup(props, { slots }) {
    return () => {
      const id = deferredIdOf(props.value)
      const hostSlots = slots as unknown as Slots
      if (id === null) {
        return h(ResolvedContent, { value: props.value, slots: hostSlots })
      }
      const resolved = props.deferred?.[String(id)]
      if (resolved !== undefined) {
        registerBoundary({
          id,
          Component: ResolvedContent,
          props: { value: resolved, slots: hostSlots },
        })
      }
      return h('ox-defer', { 'data-ox-b': String(id) }, [slots.fallback?.() ?? null])
    }
  },
})
