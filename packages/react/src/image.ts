// `<Image>` — the natural JSX wrapper for a responsive image (§16.1).
//
// A page writes either a direct URL:
//
//   <Image src="/hero.png" width={1200} height={630} sizes="100vw" priority />
//
// …or, more commonly, an imported `?w=` variant module:
//
//   import hero from './hero.png?w=400,800,1200'
//   <Image {...hero} sizes="100vw" />
//
// The `?w=` import is resolved at build time by the bundler's virtual-module
// pass: it emits `{ src, srcset, width, height }` — exactly the props this
// component consumes. The URLs point at `/_oxide/image`, which the server
// resizes at request time; no rasterization happens at build time.
//
// Behaviour:
//   * `src` + `srcset` + `sizes` land on the `<img>` verbatim;
//   * `width` / `height` are emitted when known — this is what gives the
//     browser the aspect ratio and prevents layout shift;
//   * `priority` sets `fetchpriority="high"` and, on the server, an adjacent
//     `<link rel="preload" as="image">` — a hero image should not wait for
//     the parser;
//   * `lazy` (the default) sets `loading="lazy"`; `priority` implies eager.
//
// Built with `createElement` for the same reason `<Island>` is: the props
// shape is stable across React versions and the wire output is
// byte-predictable for tests.

import { createElement, type ReactNode } from 'react'
import { deriveSizesFromSrcset } from '@oxide-fw/core/image-sizes'

/** The shape a `?w=` import produces. All fields optional so a plain `src`
 *  plus explicit `width`/`height` also satisfies the props. */
export interface ImageVariantModule {
  /** Max-width URL — the `<img src>`. */
  src?: string
  /** `"url 400w, url 800w, …"` — the `<img srcset>`. */
  srcset?: string
  /** Intrinsic width of the source (before downscaling). */
  width?: number
  /** Intrinsic height of the source. */
  height?: number
}

export interface ImageProps extends ImageVariantModule {
  /** The image URL when not using a `?w=` import. */
  src: string
  /** Overrides the variant module's `srcset`. */
  srcset?: string
  /** The `sizes` attribute — how wide the image is at each viewport. */
  sizes?: string
  /** `alt` is required for accessibility; empty string marks a decorative
   *  image (screen readers skip it). */
  alt: string
  /** Mark this image as LCP-critical: eager loading + high fetch priority +
   *  an emitted `<link rel="preload">`. */
  priority?: boolean
  /** Lazy-load. Defaults to `!priority`. */
  lazy?: boolean
  /** Extra classes for the `<img>`. */
  className?: string
  /** Anything else (id, data-*, style, …) passes through. */
  [key: string]: unknown
}

/**
 * The `<Image>` component. Returns an `<img>` (or, with `priority`, a
 * fragment of `<link rel="preload">` + `<img>` — the preload must precede
 * the tag it primes, and React permits a fragment here).
 */
export function Image(props: ImageProps): ReactNode {
  const {
    src,
    srcset,
    sizes,
    width,
    height,
    alt,
    priority = false,
    lazy,
    className,
    ...rest
  } = props

  const effectiveLazy = lazy ?? !priority
  const loading = effectiveLazy ? 'lazy' : 'eager'

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
  if (className) imgProps.className = className
  if (priority) imgProps.fetchpriority = 'high'

  const img = createElement('img', imgProps)

  if (!priority) return img

  // `<link rel="preload">` uses the first `srcset` candidate when one is
  // present (the browser picks from candidates); without a srcset it is the
  // plain `src`. `imagesizes` is required by the spec when `imagesrcset` is
  // used.
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

  return createElement(
    'Fragment',
    null,
    createElement('link', preloadProps),
    img,
  )
}
