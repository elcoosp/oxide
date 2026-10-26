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
//
// `vue` is loaded lazily (createRequire + a first-property-access Proxy) so
// importing this module does not require `vue` to be installed — the same
// "importable without the optional peer" contract the other adapter entries
// honour. Rendering the component still needs Vue at runtime.

import type { DefineComponent, PropType } from 'vue'
import { createRequire } from 'node:module'
import { registerBoundary, deferredIdOf } from '../../core/src/server/boundary.ts'

const require_ = createRequire(import.meta.url)

type Slots = Record<string, ((args: { value: unknown }) => unknown) | undefined>

function buildDefer(): DefineComponent {
  const vue = require_('vue') as typeof import('vue')
  const { defineComponent, h } = vue

  const ResolvedContent = defineComponent({
    name: 'OxideDeferredContent',
    props: {
      value: { type: null, required: false },
      slots: { type: Object as PropType<Slots>, required: false },
    },
    setup(props) {
      return () => props.slots?.default?.({ value: props.value }) ?? null
    },
  })

  return defineComponent({
    name: 'OxideDefer',
    props: {
      value: { type: null, required: false },
      deferred: { type: Object as PropType<Record<string, unknown>>, required: false },
    },
    setup(props, { slots }) {
      return () => {
        const id = deferredIdOf(props.value)
        const hostSlots: Slots = {}
        for (const [k, fn] of Object.entries(slots)) {
          if (typeof fn === 'function') {
            hostSlots[k] = (args: { value: unknown }): unknown => fn(args)
          }
        }
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
  }) as unknown as DefineComponent
}

let cached: DefineComponent | undefined
function getDefer(): DefineComponent {
  if (cached) return cached
  cached = buildDefer()
  return cached
}

export const Defer: DefineComponent = new Proxy({} as DefineComponent, {
  get(_t, prop) { return Reflect.get(getDefer() as object, prop) },
  has(_t, prop) { return Reflect.has(getDefer() as object, prop) },
  ownKeys() { return Reflect.ownKeys(getDefer() as object) },
  getOwnPropertyDescriptor(_t, prop) {
    return Reflect.getOwnPropertyDescriptor(getDefer() as object, prop)
  },
})
