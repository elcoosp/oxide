// `<Image>` — Vue adapter (§16.1). `h()` returns a VNode; a plain object.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Image } from '../src/image.ts'

interface VNodeLike {
  type: string
  props: Record<string, unknown> | null
}

test('plain src + dims emits an img VNode', () => {
  const v = Image({ src: '/a.png', width: 400, height: 300, alt: 'A' }) as VNodeLike
  assert.equal(v.type, 'img')
  assert.equal(v.props?.src, '/a.png')
  assert.equal(v.props?.width, 400)
  assert.equal(v.props?.height, 300)
  assert.equal(v.props?.alt, 'A')
  assert.equal(v.props?.loading, 'lazy')
})

test('a variant module spreads srcset through', () => {
  const v = Image({
    src: '/_oxide/image?src=hero.png&w=1200',
    srcset: '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=1200 1200w',
    sizes: '100vw',
    width: 1200,
    height: 630,
    alt: 'Hero',
  }) as VNodeLike
  assert.equal(v.props?.srcset, '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=1200 1200w')
  assert.equal(v.props?.sizes, '100vw')
})

test('priority emits [link, img] with eager + high priority', () => {
  const out = Image({
    src: '/hero.png',
    srcset: '/hero.png?w=800 800w',
    sizes: '100vw',
    alt: 'Hero',
    priority: true,
  }) as VNodeLike[]
  assert.ok(Array.isArray(out), 'priority returns an array')
  assert.equal(out.length, 2)
  assert.equal(out[0].type, 'link')
  assert.equal(out[0].props?.rel, 'preload')
  assert.equal(out[0].props?.as, 'image')
  assert.equal(out[0].props?.imagesrcset, '/hero.png?w=800 800w')
  assert.equal(out[0].props?.imagesizes, '100vw')
  assert.equal(out[1].type, 'img')
  assert.equal(out[1].props?.loading, 'eager')
  assert.equal(out[1].props?.fetchpriority, 'high')
})
