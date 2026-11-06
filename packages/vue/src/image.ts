// `<Image>` — responsive image component (§16.1), Vue adapter.
//
// Same contract as the React adapter's `<Image>`: pass a `?w=` variant
// module's `{ src, srcset, width, height }` plus `alt` and `sizes`, or a
// plain `src` with explicit dims. `priority` adds `fetchpriority="high"`,
// eager loading, and an adjacent `<link rel="preload">`.
//
// Vue has no JSX intrinsic restriction, so `h('img', …)` renders directly.

import type { VNode } from 'vue'
import { deriveSizesFromSrcset } from '@oxide-fw/core/image-sizes'

type VueH = typeof import('vue').h

// Resolve `h` through a top-level-await dynamic import — NOT
// `createRequire('node:module')`. The adapter dist is served to the BROWSER
// in dev ESM mode (and bundled for client builds); a static `node:module`
// import killed the whole client module graph there (every page 500s at
// boot). The dynamic form resolves identically in the browser (the dev
// server rewrites the bare specifier to the prebundled runtime) and in the
// SSR worker; the top-level await keeps `h` synchronously available to
// `Image()` the way the old sync `require_` was.
let cachedH: VueH | undefined
let hError: string | undefined
try {
  const mod = (await import('vue')) as { h?: VueH }
  if (typeof mod?.h !== 'function') {
    hError = 'vue: module resolved but has no h() export'
  } else {
    cachedH = mod.h
  }
} catch (e) {
  hError = e instanceof Error ? e.message : String(e)
}

function getVueH(): VueH {
  if (cachedH) return cachedH
  throw new Error(
    `@oxide-fw/vue Image: vue runtime unavailable${hError ? ` (${hError})` : ''} — add vue to the project's dependencies`,
  )
}

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

export function Image(props: ImageProps): VNode | VNode[] {
  const h = getVueH()
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
  const imgProps: Record<string, unknown> = {
    src,
    alt,
    loading,
    decoding: 'async',
    ...rest,
  }
  if (srcset) imgProps.srcset = srcset
  const effectiveSizes = sizes ?? deriveSizesFromSrcset(srcset)
  if (effectiveSizes) imgProps.sizes = effectiveSizes
  if (typeof width === 'number') imgProps.width = width
  if (typeof height === 'number') imgProps.height = height
  if (cls) imgProps.class = cls
  if (priority) imgProps.fetchpriority = 'high'

  const img = h('img', imgProps)
  if (!priority) return img

  const preloadProps: Record<string, unknown> = {
    rel: 'preload',
    as: 'image',
    href: src,
  }
  if (srcset) {
    preloadProps.imagesrcset = srcset
    const imSizes = sizes ?? deriveSizesFromSrcset(srcset)
    if (imSizes) preloadProps.imagesizes = imSizes
  }
  return [h('link', preloadProps), img]
}
