// `<Image>` — responsive image component (§16.1), Vue adapter.
//
// Same contract as the React adapter's `<Image>`: pass a `?w=` variant
// module's `{ src, srcset, width, height }` plus `alt` and `sizes`, or a
// plain `src` with explicit dims. `priority` adds `fetchpriority="high"`,
// eager loading, and an adjacent `<link rel="preload">`.
//
// Vue has no JSX intrinsic restriction, so `h('img', …)` renders directly.

import { h, type VNode } from 'vue'

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
  if (sizes) imgProps.sizes = sizes
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
    if (sizes) preloadProps.imagesizes = sizes
  }
  return [h('link', preloadProps), img]
}
