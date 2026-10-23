// Unit tests for the `sizes`-from-srcset derivation (§16.1).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { widthsFromSrcset, deriveSizesFromSrcset } from '../src/image-sizes.ts'

test('widthsFromSrcset: extracts width descriptors, descending and deduped', () => {
  const s = '/_oxide/image?src=a.png&w=400 400w, /_oxide/image?src=a.png&w=1200 1200w, /_oxide/image?src=a.png&w=800 800w'
  assert.deepEqual(widthsFromSrcset(s), [1200, 800, 400])
})

test('widthsFromSrcset: dedupes repeated widths', () => {
  assert.deepEqual(widthsFromSrcset('/a 400w, /b 400w, /c 800w'), [800, 400])
})

test('widthsFromSrcset: returns [] for density descriptors (1x/2x)', () => {
  assert.deepEqual(widthsFromSrcset('/a.png 1x, /a@2x.png 2x'), [])
})

test('widthsFromSrcset: returns [] for a bare URL', () => {
  assert.deepEqual(widthsFromSrcset('/a.png'), [])
  assert.deepEqual(widthsFromSrcset(''), [])
})

test('deriveSizesFromSrcset: every candidate gets an entry, ends at 100vw', () => {
  const s = '/_oxide/image?src=hero.png&w=400 400w, /_oxide/image?src=hero.png&w=800 800w, /_oxide/image?src=hero.png&w=1200 1200w'
  // Every candidate is reachable: a 1200px viewport picks 1200px,
  // 800-1199 picks 800px, 400-799 picks 400px, <400 falls through to
  // 100vw (which the browser resolves against the smallest candidate).
  assert.equal(
    deriveSizesFromSrcset(s),
    '(min-width: 1200px) 1200px, (min-width: 800px) 800px, (min-width: 400px) 400px, 100vw',
  )
})

test('deriveSizesFromSrcset: undefined for srcset with no width descriptors', () => {
  assert.equal(deriveSizesFromSrcset('/a 1x, /a@2x 2x'), undefined)
  assert.equal(deriveSizesFromSrcset('/a.png'), undefined)
})

test('deriveSizesFromSrcset: undefined when srcset is absent', () => {
  assert.equal(deriveSizesFromSrcset(undefined), undefined)
  assert.equal(deriveSizesFromSrcset(''), undefined)
})

test('deriveSizesFromSrcset: single-width srcset', () => {
  assert.equal(
    deriveSizesFromSrcset('/a 800w'),
    '(min-width: 800px) 800px, 100vw',
  )
})
