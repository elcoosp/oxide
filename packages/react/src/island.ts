// `<Island>` — the natural JSX wrapper for a client island (§10.3).
//
// A page writes:
//
//   <Island component="Counter" chunk="/islands/counter.js" props={{ initial: 3 }}>
//     <Counter initial={3} />
//   </Island>
//
// The component emits an `<ox-island>` custom element whose attribute names
// and value shapes match `render::island_element()` byte-for-byte (attribute
// names, not quote style — React 19 renders attributes with `"`). The
// children are rendered by the framework as normal SSR; on the client, the
// boot script (`oxide/client`) scans `<ox-island[data-ox-chunk]>`, decodes
// `data-ox-props`, dynamic-imports the chunk, and calls the adapter's
// `hydrateIsland`.
//
// We build the element with `React.createElement` (not JSX) so the type
// system doesn't reject `ox-island` as an unknown intrinsic. Runtime
// output is identical.

import { createElement, type ReactNode } from 'react'
import { toBase64 } from '@oxide-fw/core/bytes'
import { encodePropsB64 } from '@oxide-fw/core/odf-encode'

/** Hydration timing (§10.3): `load` (immediate), `idle`, `visible`, or
 *  `{ media: '(...)' }` for a CSS media query. */
export type IslandStrategy = 'load' | 'idle' | 'visible' | { media: string }

/** The adapter packages' own framework — the default for `IslandProps.framework`. */
export const REACT_FRAMEWORK = 'react'

export interface IslandProps {
  /** Stable name — must match a key in `config.islands` so the build can
   *  resolve the emitted chunk. Also the value written to `data-ox-island`
   *  (used by the client to look up the module and by the dev inspector). */
  component: string
  /** Logical chunk id (`/islands/counter.js` or `Counter`). The build
   *  rewrites this to the hashed URL emitted to `dist/client/`; the dev
   *  server resolves it live. A foreign island (see `framework`) carries
   *  none — its chunk belongs to its own adapter's build. */
  chunk?: string
  /** Hydration timing; default `'load'`. */
  strategy?: IslandStrategy
  /** Props forwarded to the client island. Serialized as base64-encoded
   *  JSON into `data-ox-props`; the client boot script decodes it. */
  props?: Record<string, unknown>
  /** The island's own framework (§V2-F01). When set and different from this
   *  adapter's framework, the component emits the SELF-CONTAINED pending
   *  placeholder — `<ox-island data-ox-island data-ox-fw data-ox-pending='1'
   *  data-ox-props>` with no rendered children — that the worker's
   *  `island:render` op + the core `resolveForeign` stream transform resolve
   *  with the island's own adapter. Children are ignored in that case (a
   *  foreign framework's children cannot be SSR'd here). */
  framework?: string
  /** Optional island placeholder markup. Omitted when the island has no
   *  fallback content — `createElement(Island, props, child)` fills it via
   *  React's trailing-children mechanism, which requires the prop to be
   *  optional for TS overload resolution (TS2769 otherwise). */
  children?: ReactNode
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

export function Island({
  component,
  chunk,
  strategy = 'load',
  props = {},
  framework,
  children,
}: IslandProps): ReactNode {
  const propsB64 = encodePropsB64(props)
  // §V2-F01 item 4: the foreign placeholder is self-contained — the pending
  // flag is what the core `resolveForeign` transform selects on, `data-ox-fw`
  // names the island's own framework for the worker's `island:render` op,
  // and no chunk/strategy attrs ride it.
  if (framework !== undefined && framework !== REACT_FRAMEWORK) {
    return createElement('ox-island', {
      suppressHydrationWarning: true,
      'data-ox-island': component,
      'data-ox-fw': framework,
      'data-ox-pending': '1',
      'data-ox-props': propsB64,
    })
  }
  return createElement(
    'ox-island',
    {
      suppressHydrationWarning: true,
      'data-ox-island': component,
      'data-ox-chunk': chunk,
      'data-ox-strategy': strategyAttr(strategy),
      'data-ox-props': propsB64,
    },
    children,
  )
}
