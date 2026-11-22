// V2-R01 C46 fixture — the typed-route overload shapes the typegen emits
// (`crates/router/src/typegen.rs` → `.oxide/types/routes.d.ts`), exercised
// against the runtime `hrefFor` binds to. This file compiles under the
// conformance tsconfig (`pnpm -C conformance typecheck` = `tsc --noEmit`);
// a bad call (a missing required param, a wrong-type param) fails tsc — the
// "rejects bad calls" half of C46. The runtime half is pinned in
// `conformance/assertions/c46.ts`.
//
// This is the POSITIVE compile surface: every call below is well-typed. A
// negative fixture (a deliberately bad call) is out of scope for a single
// `tsc --noEmit` pass — the typegen's own typecheck tests cover the negative
// arm.

import { hrefFor } from '../../packages/core/src/client/href.ts'

// Every segment spelling fills from params (the generated overloads accept
// the route's param shape).
hrefFor('/blog/[slug]', { slug: 'hello' })
hrefFor('/blog/_slug_', { slug: 'hello' })
hrefFor('/blog/:slug', { slug: 'hello' })
hrefFor('/tags/[...rest]', { rest: ['a', 'b'] })

// Leftover params ride the query; opts.search serializes.
hrefFor('/about', { q: 'x' }, { search: { page: 2 } })
hrefFor('/about', undefined, { search: { tag: ['a', 'b'] } })

// The i18n locale prefix (prefixed unless it equals the default).
hrefFor('/about', undefined, { locale: 'fr', defaultLocale: 'en' })
hrefFor('/about', undefined, { locale: 'en', defaultLocale: 'en' })

// The fragment.
hrefFor('/about', { hash: '#top' })

// Exported so the fixture is reachable from a typecheck entrypoint.
export const fixture = {
  blog: hrefFor('/blog/[slug]', { slug: 'x' }),
  about: hrefFor('/about', { q: 'x' }, { search: { page: 2 } }),
  localized: hrefFor('/about', undefined, { locale: 'fr', defaultLocale: 'en' }),
}
