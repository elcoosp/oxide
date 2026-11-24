// `Link` — the typed anchor for client navigation (§8.4 + §V2-E02), the
// Vue adapter's twin of the React adapter's `Link`.
//
// The core router intercepts same-origin anchors (and any anchor marked
// `data-ox-link`), so a typed link is an `<a>` whose `href` the runtime
// builds: the generated `href` overloads in `.oxide/types/routes.d.ts`
// are the TYPE check, `hrefFor` (the core runtime) is the builder. A page
// writes:
//
//   <Link route="/blog/[slug]" :params="{ slug: post.slug }">
//     {{ post.title }}
//   </Link>
//
// Vue has no JSX intrinsic restriction, so `h('a', …)` renders directly —
// the same choice the `Image` twin records. The `data-ox-link` attribute
// marks the link for forced interception (the router's own contract), so
// a `Link` still navigates client-side when an app's `data-ox-reload`
// opt-out or an external-origin check would otherwise skip it. The
// children come from the default slot (a component's usual SFC shape) or
// the `children` prop (a `h()`-caller's shape).

import type { VNode, VNodeChild } from 'vue'
import { hrefFor, type HrefOpts, type HrefParam } from '@oxide-fw/core/client'

type VueH = typeof import('vue').h

// Resolve `h` through a top-level-await dynamic import — the `Image`
// twin's posture: a static `node:module` require killed the client module
// graph in dev ESM mode, and the dynamic form resolves identically in the
// browser (the dev server rewrites the bare specifier to the prebundled
// runtime) and in the SSR worker.
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
    `@oxide-fw/vue Link: vue runtime unavailable${hError ? ` (${hError})` : ''} — add vue to the project's dependencies`,
  )
}

export interface LinkProps {
  /** The route pattern the generated overloads type (`/blog/[slug]`). */
  route: string
  /** The route's params — dynamic segments fill, leftovers ride the query. */
  params?: Record<string, HrefParam>
  /** The typed search object, hash, and i18n locale (the href opts). */
  opts?: HrefOpts
  /** Anchor attributes spread verbatim (`class`, `aria-*`, …). */
  anchorProps?: Record<string, string | number | boolean>
  /** The link's label, when the caller drives `h()` directly. */
  children?: VNodeChild
}

/** The default slot's render, when the caller is an SFC template. */
export interface LinkSlots {
  default?: () => VNodeChild
}

export function Link(props: LinkProps, ctx?: { slots?: LinkSlots }): VNode {
  const h = getVueH()
  const { route, params, opts, anchorProps, children } = props
  const label: VNodeChild = ctx?.slots?.default?.() ?? children
  return h(
    'a',
    {
      href: hrefFor(route, params, opts),
      'data-ox-link': true,
      ...anchorProps,
    },
    label,
  )
}

// V2-E02 re-exports: the typed navigation entries app code imports from
// the adapter (the same names the generated overloads declare).
export { hrefFor as href, goto, parseSearch } from '@oxide-fw/core/client'
export type { HrefOpts, GotoOpts, SearchParseResult } from '@oxide-fw/core/client'
