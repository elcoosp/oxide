// `<Image>` — responsive image component (§16.1).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Image } from '../src/image.ts'

/** Minimal React-element walker: our component returns createElement()
 *  trees, which are plain objects with `type` and `props`. No React runtime
 *  is needed to inspect them. */
interface El { type: unknown; props: Record<string, unknown> & { children?: unknown } }

test('plain src + dims emits an img with width/height/alt', () => {
  const el = Image({ src: '/a.png', width: 400, height: 300, alt: 'A' }) as El
  assert.equal(el.type, 'img')
  assert.equal(el.props.src, '/a.png')
  assert.equal(el.props.width, 400)
  assert.equal(el.props.height, 300)
  assert.equal(el.props.alt, 'A')
  assert.equal(el.props.loading, 'lazy')
})

test('a ?w= variant module spreads srcset through', () => {
  const variant = {
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=1200 1200w',
    width: 1200,
    height: 630,
  }
  const el = Image({ ...variant, sizes: '100vw', alt: 'Hero' }) as El
  assert.equal(el.type, 'img')
  assert.equal(el.props.srcset, variant.srcset)
  assert.equal(el.props.sizes, '100vw')
  assert.equal(el.props.width, 1200)
})

test('priority emits a preload link + eager img in a fragment', () => {
  const el = Image({
    src: '/hero.png',
    srcset: '/hero.png?w=800 800w',
    sizes: '100vw',
    alt: 'Hero',
    priority: true,
  }) as El
  assert.equal(el.type, 'Fragment')
  const kids = el.props.children as El[]
  assert.equal(kids.length, 2)
  assert.equal(kids[0].type, 'link')
  assert.equal(kids[0].props.rel, 'preload')
  assert.equal(kids[0].props.as, 'image')
  assert.equal(kids[0].props.imagesrcset, '/hero.png?w=800 800w')
  assert.equal(kids[0].props.imagesizes, '100vw')
  assert.equal(kids[1].type, 'img')
  assert.equal(kids[1].props.loading, 'eager')
  assert.equal(kids[1].props.fetchpriority, 'high')
})

test('pass-through props (id, className, data-*) survive', () => {
  const el = Image({
    src: '/x.png',
    alt: '',
    id: 'hero',
    className: 'rounded',
    'data-testid': 'img',
  }) as El
  assert.equal(el.props.id, 'hero')
  assert.equal(el.props.className, 'rounded')
  assert.equal(el.props['data-testid'], 'img')
})

// §16.1: sizes derived from srcset when absent; explicit sizes wins.
test('sizes is derived from srcset when absent', () => {
  const variant = {
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=800 800w, /_oxide/image?src=hero.png&w=1200 1200w',
    width: 1200,
    height: 630,
  }
  const out = Image({ ...variant, alt: 'Hero' }) as unknown as { props: Record<string, unknown> }
  assert.equal(
    out.props.sizes,
    '(min-width: 1200px) 1200px, (min-width: 800px) 800px, (min-width: 400px) 400px, 100vw',
  )
})
