// `Link` — the typed anchor for client navigation (§8.4 + §V2-E02), the
// Solid adapter's twin of the React adapter's `Link`.
//
// The core router intercepts same-origin anchors (and any anchor marked
// `data-ox-link`), so a typed link is an `<a>` whose `href` the runtime
// builds: the generated `href` overloads in `.oxide/types/routes.d.ts`
// are the TYPE check, `hrefFor` (the core runtime) is the builder. A page
// writes:
//
//   <Link route="/blog/[slug]" params={{ slug: post.slug }}>
//     {post.title}
//   </Link>
//
// Solid has no `h()`; its SSR element builder is `ssrElement(tag, attrs,
// children, dynamic)` from the web renderer — the same choice the
// `Image` twin records. The `data-ox-link` attribute marks the link for
// forced interception (the router's own contract), so a `Link` still
// navigates client-side when an app's `data-ox-reload` opt-out or an
// external-origin check would otherwise skip it.

import { hrefFor, type HrefOpts, type HrefParam } from '@oxide-fw/core/client'

export interface LinkProps {
  /** The route pattern the generated overloads type (`/blog/[slug]`). */
  route: string
  /** The route's params — dynamic segments fill, leftovers ride the query. */
  params?: Record<string, HrefParam>
  /** The typed search object, hash, and i18n locale (the href opts). */
  opts?: HrefOpts
  /** Anchor attributes spread verbatim (`class`, `aria-*`, …). */
  anchorProps?: Record<string, string | number | boolean>
  /** The link's children — the visible label. */
  children?: unknown
}

interface SsrElementFn {
  (tag: string, attrs: Record<string, unknown>, children: unknown, dynamic: boolean): unknown
}

// `@solidjs/web` is an optional peer — the `Image` twin's posture: the
// dynamic import resolves in the browser (the dev server rewrites the
// bare specifier to the prebundled runtime) and in the SSR worker alike;
// the top-level await keeps the result synchronously readable by `Link()`,
// preserving the "optional peer → descriptor fallback" contract.
type WebModule = { ssrElement?: SsrElementFn }
let cachedWeb: WebModule | undefined
try {
  const mod = (await import('@solidjs/web')) as WebModule | { default?: WebModule }
  const resolved = ((mod as { default?: WebModule }).default ?? mod) as WebModule
  if (resolved && typeof resolved.ssrElement === 'function') {
    cachedWeb = resolved
  }
} catch {
  // Optional peer absent — Link() falls through to the descriptor fallback.
}

function getWeb(): WebModule | undefined {
  return cachedWeb
}

export function Link(props: LinkProps): unknown {
  const { route, params, opts, anchorProps, children } = props
  const attrs: Record<string, unknown> = {
    href: hrefFor(route, params, opts),
    'data-ox-link': true,
    ...anchorProps,
  }
  const ssrElement = getWeb()?.ssrElement
  if (typeof ssrElement !== 'function') {
    // No web renderer available — return a descriptor a caller can render
    // itself; the SSR pass treats an unknown return as "nothing to render".
    return { __oxideLink: attrs, children }
  }
  return ssrElement('a', attrs, children, false)
}

// V2-E02 re-exports: the typed navigation entries app code imports from
// the adapter (the same names the generated overloads declare).
export { hrefFor as href, goto, parseSearch } from '@oxide-fw/core/client'
export type { HrefOpts, GotoOpts, SearchParseResult } from '@oxide-fw/core/client'
