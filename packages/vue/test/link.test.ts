// V2-E02 — the Vue adapter's typed `Link` pins (the F01 twins pattern).
//
// These ride CI (`pnpm --filter @oxide-fw/vue test`; the workspace's
// node_modules resolves `vue` and `@oxide-fw/core/client`) — the same
// posture the react `link.test.ts` pins record for this sandbox. Written
// against the same contract the core `search-params`/`hrefFor` pins
// prove: `hrefFor` builds the href (params fill segments, leftovers ride
// the query, opts.search serializes, hash appends, locale prefixes), and
// the rendered anchor carries `data-ox-link` so the router intercepts it.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Link } from '../src/link.ts'

interface VNodeLike {
  type: string
  props: Record<string, unknown> | null
  children?: unknown
}

function anchor(vnode: unknown): VNodeLike {
  return vnode as VNodeLike
}

test('Link: renders an anchor with the built href and the interception mark', () => {
  const v = anchor(Link({ route: '/blog/[slug]', params: { slug: 'hi' }, children: 'Read' }))
  assert.equal(v.type, 'a')
  assert.equal(v.props?.['href'], '/blog/hi')
  assert.equal(v.props?.['data-ox-link'], true)
  assert.equal(v.children, 'Read')
})

test('Link: the search and hash opts ride the href', () => {
  const v = anchor(
    Link({
      route: '/about',
      opts: { search: { page: 2 }, hash: 'top' },
      children: 'About',
    }),
  )
  assert.equal(v.props?.['href'], '/about?page=2#top')
})

test('Link: anchor props spread verbatim after the built ones', () => {
  const v = anchor(
    Link({
      route: '/about',
      anchorProps: { class: 'nav', 'aria-label': 'About page' },
      children: 'About',
    }),
  )
  assert.equal(v.props?.['class'], 'nav')
  assert.equal(v.props?.['aria-label'], 'About page')
  assert.equal(v.props?.['href'], '/about', 'the built href survives the spread')
})

test('Link: the default slot renders the children when no children prop is given', () => {
  const slotChild = { type: 'span', props: null, children: null }
  const v = anchor(
    Link({ route: '/about' }, { slots: { default: () => slotChild } }),
  )
  assert.equal(v.children, slotChild)
})
