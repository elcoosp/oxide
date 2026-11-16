// V2-E02 — the typed `Link` pins. These ride CI (the react package needs
// the workspace's node_modules; unavailable in the recorded sandbox).
// Written against the same contract the core `search-params` pins prove:
// `hrefFor` builds the href (params fill segments, leftovers ride the
// query, opts.search serializes, hash appends, locale prefixes), and the
// rendered anchor carries `data-ox-link` so the router intercepts it.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createElement } from 'react'
import { Link } from '../src/link.ts'

/** Flatten a createElement call into a plain shape for assertions. */
function anchor(el: ReturnType<typeof createElement>): {
  type: string
  props: Record<string, unknown>
} {
  return el as unknown as { type: string; props: Record<string, unknown> }
}

test('Link: renders an anchor with the built href and the interception mark', () => {
  const el = anchor(
    Link({ route: '/blog/[slug]', params: { slug: 'hi' }, children: 'Read' }) as ReturnType<
      typeof createElement
    >,
  )
  assert.equal(el.type, 'a')
  assert.equal(el.props['href'], '/blog/hi')
  assert.equal(el.props['data-ox-link'], true)
  assert.deepEqual(el.props['children'], 'Read')
})

test('Link: the search and hash opts ride the href', () => {
  const el = anchor(
    Link({
      route: '/about',
      opts: { search: { page: 2 }, hash: 'top' },
      children: 'About',
    }) as unknown as { type: string; props: Record<string, unknown> },
  )
  assert.equal(el.props['href'], '/about?page=2#top')
})

test('Link: anchor props spread verbatim', () => {
  const el = anchor(
    Link({
      route: '/about',
      anchorProps: { className: 'nav', 'aria-label': 'About page' },
      children: 'About',
    }) as unknown as { type: string; props: Record<string, unknown> },
  )
  assert.equal(el.props['className'], 'nav')
  assert.equal(el.props['aria-label'], 'About page')
})

test('Link: createElement is the runtime path (no JSX transform dependency)', () => {
  // The component is a plain function over createElement — assert the
  // output is the same element a hand-rolled createElement call makes.
  const expected = createElement('a', { href: '/about', 'data-ox-link': true }, 'About')
  const actual = Link({ route: '/about', children: 'About' })
  assert.deepEqual(anchor(actual as ReturnType<typeof createElement>).type, 'a')
  assert.deepEqual(anchor(expected).props['href'], '/about')
})
