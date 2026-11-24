// V2-E02 — the Solid adapter's typed `Link` pins (the F01 twins pattern).
//
// These ride CI (`pnpm --filter @oxide-fw/solid test`; the workspace's
// node_modules resolves `@solidjs/web` and `@oxide-fw/core/client`) — the
// same posture the react `link.test.ts` pins record for this sandbox.
// Written against the same contract the core `hrefFor` pins prove, plus
// the adapter's own optional-peer contract: without the web renderer the
// component returns the descriptor fallback (the `Image` twin's shape).

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Link } from '../src/link.ts'

interface SsrElementLike {
  t: string
  // Solid's ssrElement result: the tag string plus spread data the SSR
  // pass consumes. The shape is asserted loosely — the interesting bits
  // are the attributes in the HTML string.
  [key: string]: unknown
}

function rendered(out: unknown): SsrElementLike {
  return out as SsrElementLike
}

test('Link: renders an anchor with the built href and the interception mark', () => {
  const out = rendered(
    Link({ route: '/blog/[slug]', params: { slug: 'hi' }, children: 'Read' }),
  )
  assert.equal(typeof out.t, 'string', 'ssrElement returns a tag string')
  assert.match(out.t, /<a/)
  assert.match(out.t, /href="\/blog\/hi"/)
  assert.match(out.t, /data-ox-link/)
  assert.match(out.t, /Read/)
})

test('Link: the search and hash opts ride the href', () => {
  const out = rendered(
    Link({
      route: '/about',
      opts: { search: { page: 2 }, hash: 'top' },
      children: 'About',
    }),
  )
  assert.match(out.t, /href="\/about\?page=2#top"/)
})

test('Link: anchor props spread verbatim after the built ones', () => {
  const out = rendered(
    Link({
      route: '/about',
      anchorProps: { class: 'nav', 'aria-label': 'About page' },
      children: 'About',
    }),
  )
  assert.match(out.t, /class="nav"/)
  assert.match(out.t, /aria-label="About page"/)
  assert.match(out.t, /href="\/about"/, 'the built href survives the spread')
})
