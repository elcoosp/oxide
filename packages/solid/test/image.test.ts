// `<Image>` — Solid adapter (§16.1). Uses the web renderer's `ssrElement`.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Image } from '../src/image.ts'

interface SsrElementLike {
  t: string
}

test('plain src + dims renders an img descriptor', () => {
  const out = Image({ src: '/a.png', width: 400, height: 300, alt: 'A' }) as SsrElementLike
  assert.equal(typeof out.t, 'string')
  assert.match(out.t, /<img/, out.t)
  assert.match(out.t, /src="\/a\.png"/, out.t)
  assert.match(out.t, /width="400"/, out.t)
  assert.match(out.t, /height="300"/, out.t)
  assert.match(out.t, /alt="A"/, out.t)
  assert.match(out.t, /loading="lazy"/, out.t)
})

test('a variant module spreads srcset through', () => {
  const out = Image({
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=1200 1200w',
    sizes: '100vw',
    width: 1200,
    height: 630,
    alt: 'Hero',
  }) as SsrElementLike
  assert.match(out.t, /srcset="[^"]*400w[^"]*1200w"/, out.t)
  assert.match(out.t, /sizes="100vw"/, out.t)
})

test('priority loads eagerly and sets fetchpriority', () => {
  const out = Image({ src: '/hero.png', alt: 'Hero', priority: true }) as SsrElementLike
  assert.match(out.t, /loading="eager"/, out.t)
  assert.match(out.t, /fetchpriority="high"/, out.t)
})

// §16.1: sizes derived from srcset when absent.
test('sizes is derived from srcset when absent', () => {
  const out = Image({
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=800 800w, /_oxide/image?src=hero.png&w=1200 1200w',
    width: 1200,
    height: 630,
    alt: 'Hero',
  })
  const t = typeof out === 'string' ? out : (out as { t: string }).t
  assert.match(t, /sizes="\(min-width: 1200px\) 1200px, \(min-width: 800px\) 800px, \(min-width: 400px\) 400px, 100vw"/, t)
})
