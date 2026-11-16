// `<Island>` — the Vue adapter's island wrapper (§10.3, §V2-F01).
//
// The twin of `packages/react/src/island.ts`: a page writes
//
//   <Island component="Counter" chunk="/islands/counter.js" :props="{ initial: 3 }">
//     <Counter :initial="3" />
//   </Island>
//
// The component emits an `<ox-island>` custom element whose attribute names
// and value shapes match `render::island_element()` byte-for-byte (attribute
// names, not quote style — Vue 3 renders attributes with `"`). The children
// render through Vue's normal slots; on the client, the boot script scans
// `<ox-island[data-ox-chunk]>`, decodes `data-ox-props`, dynamic-imports the
// chunk and calls the adapter's `hydrateIsland`.
//
// §V2-F01 item 4: a FOREIGN island (the `framework` prop naming another
// framework) emits the self-contained pending placeholder the worker's
// `island:render` op + the core `resolveForeign` transform resolve with the
// island's own adapter — no chunk/strategy attrs, children ignored (a
// foreign framework's children cannot be SSR'd by this adapter).

import { h, type VNode } from 'vue'
import { encodePropsB64 } from '@oxide-fw/core/odf-encode'

/** The adapter packages' own framework — the default for `IslandProps.framework`. */
export const VUE_FRAMEWORK = 'vue'

/** Hydration timing (§10.3): `load` (immediate), `idle`, `visible`, or
 *  `{ media: '(...)' }` for a CSS media query. */
export type IslandStrategy = 'load' | 'idle' | 'visible' | { media: string }

export interface IslandProps {
  /** Stable name — must match a key in `config.islands` so the build can
   *  resolve the emitted chunk. Also the value written to `data-ox-island`. */
  component: string
  /** Logical chunk id (`/islands/counter.vue` or `Counter`). The build
   *  rewrites this to the hashed URL emitted to `dist/client/`; the dev
   *  server resolves it live. A foreign island (see `framework`) carries
   *  none — its chunk belongs to its own adapter's build. */
  chunk?: string
  /** Hydration timing; default `'load'`. */
  strategy?: IslandStrategy
  /** Props forwarded to the client island. Serialized as base64-encoded
   *  JSON (ODF) into `data-ox-props`; the client boot script decodes it. */
  props?: Record<string, unknown>
  /** The island's own framework (§V2-F01). When set and different from this
   *  adapter's framework, the component emits the self-contained pending
   *  placeholder. */
  framework?: string
}

function strategyAttr(s: IslandStrategy): string {
  if (typeof s === 'string') return s
  // Accept both `'(min-width: 800px)'` (the CSS reading) and
  // `'min-width: 800px'` (no parens). The wire shape is always
  // `media(<inner>)`.
  const inner = s.media.startsWith('(') && s.media.endsWith(')')
    ? s.media.slice(1, -1)
    : s.media
  return `media(${inner})`
}

/** The functional `<Island>` component. Render with `h(Island, props,
 *  children)` (a default-slot shorthand) or a slot object. */
export function Island(props: IslandProps, ctx: { slots?: { default?: () => VNode[] | VNode | null } }): VNode {
  const {
    component,
    chunk,
    strategy = 'load',
    props: islandProps = {},
    framework,
  } = props
  const propsB64 = encodePropsB64(islandProps)
  // §V2-F01 item 4: the foreign placeholder is self-contained — the pending
  // flag is what the core `resolveForeign` transform selects on, `data-ox-fw`
  // names the island's own framework for the worker's `island:render` op,
  // and no chunk/strategy attrs ride it.
  if (framework !== undefined && framework !== VUE_FRAMEWORK) {
    return h('ox-island', {
      'data-ox-island': component,
      'data-ox-fw': framework,
      'data-ox-pending': '1',
      'data-ox-props': propsB64,
    })
  }
  const children = ctx.slots?.default?.()
  return h(
    'ox-island',
    {
      'data-ox-island': component,
      'data-ox-chunk': chunk,
      'data-ox-strategy': strategyAttr(strategy),
      'data-ox-props': propsB64,
    },
    children,
  )
}
