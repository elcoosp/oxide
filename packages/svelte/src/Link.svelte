<!-- `Link` — the typed anchor for client navigation (§8.4 + §V2-E02), the
     Svelte adapter's twin of the React adapter's `Link`.
     -->
<!--
     The core router intercepts same-origin anchors (and any anchor marked
     `data-ox-link`), so a typed link is an `<a>` whose `href` the runtime
     builds: the generated `href` overloads in `.oxide/types/routes.d.ts`
     are the TYPE check, `hrefFor` (the core runtime) is the builder. A page
     writes:

       <Link route="/blog/[slug]" params={{ slug: post.slug }}>
         {post.title}
       </Link>

     The `data-ox-link` attribute marks the link for forced interception
     (the router's own contract), so a `Link` still navigates client-side
     when an app's `data-ox-reload` opt-out or an external-origin check
     would otherwise skip it. The typed navigation re-exports (`href`,
     `goto`, `parseSearch`) live in `src/link.ts` — an SFC cannot re-export
     module members — exported from the package root beside this file's
     `./link` subpath.
     -->
<script lang="ts">
  import { hrefFor, type HrefOpts, type HrefParam } from '@oxide-fw/core/client'

  interface AnchorProps {
    [key: string]: string | number | boolean
  }

  interface Props {
    /** The route pattern the generated overloads type (`/blog/[slug]`). */
    route: string
    /** The route's params — dynamic segments fill, leftovers ride the query. */
    params?: Record<string, HrefParam>
    /** The typed search object, hash, and i18n locale (the href opts). */
    opts?: HrefOpts
    /** Anchor attributes spread verbatim (`class`, `aria-*`, …). */
    anchorProps?: AnchorProps
    /** The link's children — the visible label (a snippet). */
    children?: import('svelte').Snippet
  }

  let { route, params, opts, anchorProps = {}, children }: Props = $props()

  const href = $derived(hrefFor(route, params, opts))
</script>

<a {...anchorProps} {href} data-ox-link="true">
  {@render children?.()}
</a>
