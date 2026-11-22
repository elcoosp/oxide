// §V2-R01 C46 — typed route fixture compiles + rejects bad calls (E02).
//
// Pins the RUNTIME of the generated typed-route overloads
// (`packages/core/src/client/href.ts::hrefFor` — the impl `crates/router/src/
// typegen.rs`'s `.oxide/types/routes.d.ts` overloads bind to): segment
// filling for every spelling (`[name]`, `_name_`, `:name`, `[...rest]`),
// leftovers ride the query, the i18n locale prefix, and `opts.search`/`hash`.
// The "compiles + rejects bad calls" half is the TYPE check: the generated
// overloads reject a bad call at `tsc` time. The fixture
// `conformance/fixtures/typed-route-fixture.ts` exercises the overload shapes
// and rides the `conformance typecheck` (tsc --noEmit) CI step — the
// positive compile half. The runtime never throws (leniency: a missing param
// leaves the token — a visible bug, not a crashed page).

import { hrefFor } from '../../packages/core/src/client/href.ts'
import type { Assertion } from './types.ts'

function assertEq(actual: string, expected: string, label: string): void {
  if (actual !== expected) {
    throw new Error(`${label}: got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`)
  }
}

const c46: Assertion = {
  id: 'C46',
  name: 'typed route fixture compiles + rejects bad calls (E02)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  run() {
    // Every segment spelling the build emits fills from params.
    assertEq(hrefFor('/blog/[slug]', { slug: 'hello' }), '/blog/hello', '[name] segment')
    assertEq(hrefFor('/blog/_slug_', { slug: 'hello' }), '/blog/hello', '_name_ segment (manifest form)')
    assertEq(hrefFor('/blog/:slug', { slug: 'hello' }), '/blog/hello', ':name segment (hand-written)')
    assertEq(hrefFor('/tags/[...rest]', { rest: ['a', 'b'] }), '/tags/a/b', '[...rest] array joined by /')

    // A param the route does not consume rides the query string.
    assertEq(hrefFor('/about', { q: 'x' }, { search: { page: 2 } }), '/about?q=x&page=2', 'leftover + opts.search')
    // opts.search with arrays repeats the key; null is skipped.
    assertEq(hrefFor('/about', undefined, { search: { tag: ['a', 'b'], skip: null } }), '/about?tag=a&tag=b', 'search arrays repeat, null skipped')

    // The i18n locale prefix — prefixed unless it equals the default.
    assertEq(hrefFor('/about', undefined, { locale: 'fr', defaultLocale: 'en' }), '/fr/about', 'locale prefixed')
    assertEq(hrefFor('/about', undefined, { locale: 'en', defaultLocale: 'en' }), '/about', 'default locale stays unprefixed')

    // The fragment half — `#…` with the leading `#` normalized.
    assertEq(hrefFor('/about', { hash: '#top' }), '/about#top', 'opts.hash appends #…')
    assertEq(hrefFor('/about', { hash: 'top' }), '/about#top', 'hash without leading # is normalized')

    // Leniency: a declared-but-omitted param leaves the token verbatim (the
    // generated overload is the TYPE check; the runtime never throws).
    assertEq(hrefFor('/blog/[slug]', {}), '/blog/[slug]', 'missing param leaves the token')

    // The fixture exercises the overload shapes the typegen emits; its
    // `tsc --noEmit` compile (the "rejects bad calls" half) rides the
    // `conformance typecheck` CI step — recorded, not deferred.
    return {
      ok: true,
      detail: 'hrefFor fills [name]/_name_/:name/[...rest] + leftover→query + locale + hash; type-rejection rides conformance typecheck',
    }
  },
}

export default c46
