// `<Image>` — responsive image component (§16.1), Vue adapter.
//
// Same contract as the React adapter's `<Image>`: pass a `?w=` variant
// module's `{ src, srcset, width, height }` plus `alt` and `sizes`, or a
// plain `src` with explicit dims. `priority` adds `fetchpriority="high"`,
// eager loading, and an adjacent `<link rel="preload">`.
//
// Vue has no JSX intrinsic restriction, so `h('img', …)` renders directly.

import type { VNode } from 'vue'
import { createRequire } from 'node:module'
import { deriveSizesFromSrcset } from '@oxide-fw/core/image-sizes'

const require_ = createRequire(import.meta.url)
type VueH = typeof import('vue').h
let cachedH: VueH | undefined
function getVueH(): VueH {
  if (cachedH) return cachedH
  cachedH = (require_('vue') as { h: VueH }).h
  return cachedH
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
