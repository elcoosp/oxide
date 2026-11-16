// `Link` — the typed anchor for client navigation (§8.4 + §V2-E02).
//
// The core router intercepts same-origin anchors (and any anchor marked
// `data-ox-link`), so a typed link is an `<a>` whose `href` the runtime
// builds: the generated `href` overloads in `.oxide/types/routes.d.ts`
// are the TYPE check, `hrefFor` (the core runtime) is the builder. A page
// writes:
//
//   <Link route="/blog/[slug]" params={{ slug: post.slug }} search={{ page: 2 }}>
//     {post.title}
//   </Link>
//
// The component renders through `createElement` (not JSX) so the type
// system doesn't reject `data-ox-link` or spread anchor props — the same
// choice `island.ts` records. The `data-ox-link` attribute marks the link
// for forced interception (the router's own contract), so a `Link` still
// navigates client-side when an app's `data-ox-reload` opt-out or an
// external-origin check would otherwise skip it.

import { createElement, type ReactNode } from 'react'
import { hrefFor, type HrefOpts, type HrefParam } from '@oxide-fw/core/client'

export interface LinkProps {
  /** The route pattern the generated overloads type (`/blog/[slug]`). */
  route: string
  /** The route's params — dynamic segments fill, leftovers ride the query. */
  params?: Record<string, HrefParam>
  /** The typed search object, hash, and i18n locale (the href opts). */
  opts?: HrefOpts
  /** Anchor attributes spread verbatim (`className`, `aria-*`, …). */
  anchorProps?: Record<string, string | number | boolean>
  /** The link's children — the visible label. */
  children?: ReactNode
}

export function Link({ route, params, opts, anchorProps, children }: LinkProps): ReactNode {
  return createElement(
    'a',
    {
      href: hrefFor(route, params, opts),
      'data-ox-link': true,
      ...anchorProps,
    },
    children,
  )
}

// V2-E02 re-exports: the typed navigation entries app code imports from
// the adapter (the same names the generated overloads declare).
export { hrefFor as href, goto, parseSearch } from '@oxide-fw/core/client'
export type { HrefOpts, GotoOpts, SearchParseResult } from '@oxide-fw/core/client'
