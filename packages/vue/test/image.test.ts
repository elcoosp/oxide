// `<Image>` — Vue adapter (§16.1). `h()` returns a VNode; a plain object.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Image } from '../src/image.ts'

interface VNodeLike {
  type: string
  props: Record<string, unknown> | null
}

/** Typed indexed access with a runtime bounds check. Throws a clear error
 * (failing the test) instead of returning `T | undefined` — the honest way
 * to satisfy `noUncheckedIndexedAccess` in tests whose fixtures are known
 * non-empty. */
function mustEl<T>(arr: readonly T[], i: number): T {
  const v = arr[i]
  if (v === undefined) throw new Error(`mustEl: index ${i} out of bounds (len=${arr.length})`)
  return v
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
  assert.equal(mustEl(out, 0).type, 'link')
  assert.equal(mustEl(out, 0).props?.rel, 'preload')
  assert.equal(mustEl(out, 0).props?.as, 'image')
  assert.equal(mustEl(out, 0).props?.imagesrcset, '/hero.png?w=800 800w')
  assert.equal(mustEl(out, 0).props?.imagesizes, '100vw')
  assert.equal(mustEl(out, 1).type, 'img')
  assert.equal(mustEl(out, 1).props?.loading, 'eager')
  assert.equal(mustEl(out, 1).props?.fetchpriority, 'high')
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
