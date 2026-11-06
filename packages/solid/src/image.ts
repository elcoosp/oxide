// `<Image>` — responsive image component (§16.1), Solid adapter.
//
// Solid has no `h()`; its SSR element builder is `ssrElement(tag, attrs,
// children, dynamic)` from the web renderer. A component returns that
// descriptor, which the SSR pass renders to `<img …>`.
//
// Same contract as the other adapters: a `?w=` variant module or a plain
// `src` + dims. `priority` marks the image eager + high-priority. The
// `<link rel="preload">` half is not emitted here — Solid's SSR pass
// consumes a single element descriptor per component, and a fragment of
// two would need a wrapper component. A page that wants the preload adds
// it via the adapter's `head()`; documented in the compat notes.

import { deriveSizesFromSrcset } from '@oxide-fw/core/image-sizes'

export interface ImageVariantModule {
  src?: string
  srcset?: string
  width?: number
  height?: number
}

export interface ImageProps extends ImageVariantModule {
  src: string
  srcset?: string
  sizes?: string
  alt: string
  priority?: boolean
  lazy?: boolean
  class?: string
  [key: string]: unknown
}

interface SsrElementFn {
  (tag: string, attrs: Record<string, unknown>, children: unknown, dynamic: boolean): unknown
}

// `@solidjs/web` is an optional peer. Resolve it lazily at module load —
// WITHOUT `createRequire('node:module')`: the adapter dist is served to the
// BROWSER in dev ESM mode (and bundled for client builds), where a static
// `node:module` import killed the whole client module graph (every page 500s
// at boot). The dynamic import resolves in the browser (the dev server
// rewrites the bare specifier to the prebundled runtime) and in the SSR
// worker alike; the top-level await keeps the result synchronously readable
// by `Image()`, preserving the "optional peer → descriptor fallback"
// contract.
type WebModule = { ssrElement?: SsrElementFn }
let cachedWeb: WebModule | undefined
try {
  const mod = (await import('@solidjs/web')) as WebModule | { default?: WebModule }
  const resolved = ((mod as { default?: WebModule }).default ?? mod) as WebModule
  if (resolved && typeof resolved.ssrElement === 'function') {
    cachedWeb = resolved
  }
} catch {
  // Optional peer absent — Image() falls through to the descriptor fallback.
}

function getWeb(): WebModule | undefined {
  return cachedWeb
}

export function Image(props: ImageProps): unknown {
  const {
    src,
    srcset,
    sizes,
    width,
    height,
    alt,
    priority = false,
    lazy,
    class: cls,
    ...rest
  } = props

  const loading = (lazy ?? !priority) ? 'lazy' : 'eager'
  const attrs: Record<string, unknown> = {
    src,
    alt,
    loading,
    decoding: 'async',
    ...rest,
  }
  if (srcset) attrs.srcset = srcset
  const effectiveSizes = sizes ?? deriveSizesFromSrcset(srcset)
  if (effectiveSizes) attrs.sizes = effectiveSizes
  if (typeof width === 'number') attrs.width = width
  if (typeof height === 'number') attrs.height = height
  if (cls) attrs.class = cls
  if (priority) attrs.fetchpriority = 'high'

  const ssrElement = getWeb()?.ssrElement
  if (typeof ssrElement !== 'function') {
    // No web renderer available — return a descriptor a caller can render
    // itself; the SSR pass treats an unknown return as "nothing to render".
    return { __oxideImage: attrs }
  }
  return ssrElement('img', attrs, undefined, false)
}
