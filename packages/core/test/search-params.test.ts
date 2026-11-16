// V2-E02 — the client half pins (typed search params, typed `href` and
// `goto`). Written FIRST (TDD red) against the missing
// `packages/core/src/client/search-params.ts` + `href.ts` modules.
//
// The contract (spec §V2-E02, the runtime half of the landed typegen):
// - `parseSearch(schema, params)` calls the schema's Standard Schema V1
//   `~standard.validate` and surfaces `issues` under the spec's
//   `OX-SRC-001` code — the caller decides the 400 page; when validation
//   CHANGED the value (a default applied, a coercion re-serialized), the
//   result carries `corrected` — the query string the caller may redirect
//   to so the URL matches what the schema produced.
// - `hrefFor(route, …)` is the RUNTIME of the generated `href` overloads
//   (`.oxide/types/routes.d.ts`): params fill the dynamic segments
//   (`[slug]`, the manifest's `_slug_`, `:slug`, `[...rest]`), leftover
//   params ride the query string, `opts.search` serializes as the query,
//   `opts.hash` appends `#…`, and an i18n `locale` prefixes `/{locale}`
//   (the `prerendered/{locale}/…` convention) unless it is the default.
// - `goto(router, route, …)` builds the same href and hands it to the
//   router's `goto` — the typed navigation entry the adapters re-export.
//
// All schemas in these pins are structural Standard Schema V1 fakes — the
// runtime only ever touches `['~standard'].validate`, never a concrete
// validator, so no dependency is needed to pin the contract.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { parseSearch, type StandardSchemaV1Like } from '../src/client/search-params.ts'
import { hrefFor, goto } from '../src/client/href.ts'

// ------------------------------------------------------ the fake schemas

/** A schema that validates `q` is present and coerces `page` to a number. */
function qSchema(): StandardSchemaV1Like<Record<string, string>, { q: string; page: number }> {
  return {
    '~standard': {
      validate(value: unknown) {
        const v = value as Record<string, string>
        if (!v || typeof v.q !== 'string' || v.q === '') {
          return { issues: [{ message: 'q is required' }] }
        }
        return { value: { q: v.q, page: Number(v.page ?? '1') } }
      },
    },
  }
}

/** A schema with a DEFAULT: a missing `lang` comes back as `'en'`. */
function defaultsSchema(): StandardSchemaV1Like<Record<string, string>, { lang: string }> {
  return {
    '~standard': {
      validate(value: unknown) {
        const v = (value ?? {}) as Record<string, string>
        return { value: { lang: v.lang ?? 'en' } }
      },
    },
  }
}

/** An async schema (the Standard contract allows a promise result). */
function asyncSchema(): StandardSchemaV1Like<Record<string, string>, { q: string }> {
  return {
    '~standard': {
      validate(value: unknown) {
        const v = value as Record<string, string>
        return Promise.resolve(
          typeof v?.q === 'string' && v.q !== ''
            ? { value: { q: v.q } }
            : { issues: [{ message: 'async: q is required' }] },
        )
      },
    },
  }
}

// ------------------------------------------------------ parseSearch

test('parseSearch: a conforming search returns the validated value', () => {
  const res = parseSearch(qSchema(), new URLSearchParams('q=oxide&page=3'))
  assert.deepEqual(res, { ok: true, search: { q: 'oxide', page: 3 }, corrected: undefined })
})

test('parseSearch: issues surface under OX-SRC-001', () => {
  const res = parseSearch(qSchema(), new URLSearchParams('page=2'))
  assert.equal(res.ok, false)
  if (!res.ok) {
    assert.equal(res.code, 'OX-SRC-001')
    assert.deepEqual(res.issues, [{ message: 'q is required' }])
  }
})

test('parseSearch: an async schema validates through the promise', async () => {
  const ok = await parseSearch(asyncSchema(), new URLSearchParams('q=hi'))
  assert.deepEqual(ok, { ok: true, search: { q: 'hi' }, corrected: undefined })
  const bad = await parseSearch(asyncSchema(), new URLSearchParams())
  assert.equal(bad.ok, false)
  if (!bad.ok) assert.equal(bad.code, 'OX-SRC-001')
})

test('parseSearch: a plain record input rides the same contract', () => {
  const res = parseSearch(qSchema(), { q: 'x', page: '2' })
  assert.deepEqual(res, { ok: true, search: { q: 'x', page: 2 }, corrected: undefined })
})

test('parseSearch: a default the URL omitted yields the corrected query string', () => {
  const res = parseSearch(defaultsSchema(), new URLSearchParams())
  assert.equal(res.ok, true)
  if (res.ok) assert.equal(res.corrected, 'lang=en')
})

test('parseSearch: no corrected URL when validation is value-preserving', () => {
  const res = parseSearch(defaultsSchema(), new URLSearchParams('lang=fr'))
  assert.equal(res.ok, true)
  if (res.ok) assert.equal(res.corrected, undefined)
})

test('parseSearch: repeated keys collapse last-wins into the input object', () => {
  const res = parseSearch(qSchema(), new URLSearchParams('q=a&q=b'))
  assert.equal(res.ok, true)
  if (res.ok) assert.deepEqual(res.search, { q: 'b', page: 1 })
})

// ------------------------------------------------------ hrefFor

test('hrefFor: a static route passes through verbatim', () => {
  assert.equal(hrefFor('/about'), '/about')
})

test('hrefFor: params fill every dynamic-segment spelling', () => {
  assert.equal(hrefFor('/blog/[slug]', { slug: 'hi' }), '/blog/hi')
  assert.equal(hrefFor('/blog/_slug_', { slug: 'hi' }), '/blog/hi')
  assert.equal(hrefFor('/blog/:slug', { slug: 'hi' }), '/blog/hi')
})

test('hrefFor: a rest param takes its array joined by slashes', () => {
  assert.equal(hrefFor('/docs/[...rest]', { rest: ['a', 'b'] }), '/docs/a/b')
})

test('hrefFor: a missing param leaves the token verbatim (the overload is the type check)', () => {
  assert.equal(hrefFor('/blog/[slug]', {}), '/blog/[slug]')
})

test('hrefFor: params the route does not consume ride the query string', () => {
  assert.equal(hrefFor('/about', { utm: 'x' }), '/about?utm=x')
})

test('hrefFor: opts.search serializes as the query', () => {
  assert.equal(hrefFor('/about', { search: { q: 'oxide', page: 2 } }), '/about?q=oxide&page=2')
})

test('hrefFor: opts.search arrays repeat the key', () => {
  assert.equal(hrefFor('/about', { search: { tag: ['a', 'b'] } }), '/about?tag=a&tag=b')
})

test('hrefFor: opts.hash appends the fragment (with or without the leading #)', () => {
  assert.equal(hrefFor('/about', { hash: 'sec' }), '/about#sec')
  assert.equal(hrefFor('/about', { hash: '#sec' }), '/about#sec')
})

test('hrefFor: an i18n locale prefixes /{locale} (the prerendered convention)', () => {
  assert.equal(hrefFor('/about', undefined, 'fr'), '/fr/about')
})

test('hrefFor: a locale equal to the default stays unprefixed', () => {
  assert.equal(hrefFor('/about', undefined, { locale: 'en', defaultLocale: 'en' }), '/about')
  assert.equal(hrefFor('/about', undefined, { locale: 'fr', defaultLocale: 'en' }), '/fr/about')
})

test('hrefFor: params + search + hash + locale compose', () => {
  assert.equal(
    hrefFor('/blog/[slug]', { slug: 'hi', ref: 'r' }, { search: { page: 2 }, hash: 'top' }, 'fr'),
    '/fr/blog/hi?ref=r&page=2#top',
  )
})

// ------------------------------------------------------ goto

test('goto: builds the href and hands it to the router', () => {
  const calls: Array<{ href: string; nav: unknown }> = []
  const router = { goto: (href: string, nav: unknown) => void calls.push({ href, nav }) }
  goto(router, '/blog/[slug]', { slug: 'hi' }, { search: { page: 2 } })
  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.href, '/blog/hi?page=2')
  assert.deepEqual(calls[0]?.nav, { replace: false })
})

test('goto: the replace option rides the navigation', () => {
  const calls: Array<{ href: string; nav: unknown }> = []
  const router = { goto: (href: string, nav: unknown) => void calls.push({ href, nav }) }
  goto(router, '/about', undefined, { replace: true })
  assert.equal(calls[0]?.href, '/about')
  assert.deepEqual(calls[0]?.nav, { replace: true })
})
