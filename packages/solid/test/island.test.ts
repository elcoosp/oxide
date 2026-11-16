// `<Island>` (solid) — the Solid adapter's island wrapper (§V2-F01): the
// twin of `packages/react/src/island.ts`, emitting the exact `<ox-island>`
// shape the render crate emits and the client boot script scans for, and the
// self-contained pending placeholder for a FOREIGN island (the worker's
// `island:render` op + the core `resolveForeign` transform resolve it with
// the island's own adapter).
//
// The twin is built with the SSR-flavored jsx runtime's `ssrElement` — the
// same builder `babel-preset-solid`'s `generate: 'ssr'` output imports, so
// the emitted markup is the one production SSR produces. The test renders
// through the real `@solidjs/web` `renderToString`.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { renderToString, ssr, ssrElement } from '@solidjs/web'
import { encodePropsB64 } from '../../core/src/odf-encode.ts'
import { Island } from '../src/island.ts'

test('Island (solid) emits the render-crate ox-island shape with children', async () => {
  const html = await renderToString(() =>
    Island({
      component: 'Counter',
      chunk: '/islands/counter.tsx',
      strategy: 'load',
      props: { initial: 3 },
      children: ssr('<button><span>3</span></button>'),
    }),
  )
  assert.match(html, /^<ox-island\b/)
  assert.match(html, /data-ox-island="Counter"/)
  assert.match(html, /data-ox-chunk="\/islands\/counter\.tsx"/)
  assert.match(html, /data-ox-strategy="load"/)
  const want = encodePropsB64({ initial: 3 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(html, new RegExp(`data-ox-props="${want}"`))
  assert.match(html, /<button><span>3<\/span><\/button>/)
})

test('Island (solid) default strategy is "load"', async () => {
  const html = await renderToString(() =>
    Island({ component: 'X', chunk: '/x.js', children: ssr('<span>x</span>') }),
  )
  assert.match(html, /data-ox-strategy="load"/)
})

test('Island (solid) media strategy serializes as media(...)', async () => {
  const html = await renderToString(() =>
    Island({
      component: 'X',
      chunk: '/x.js',
      strategy: { media: '(min-width: 800px)' },
      children: ssr('<span>x</span>'),
    }),
  )
  assert.match(html, /data-ox-strategy="media\(min-width: 800px\)"/)
})

test('Island (solid) emits the pending placeholder for a foreign framework island', async () => {
  const html = await renderToString(() =>
    Island({
      component: 'Price',
      chunk: '/islands/Price.vue',
      framework: 'vue',
      props: { a: 1 },
      children: ssr('<span>ignored</span>'),
    }),
  )
  assert.match(html, /^<ox-island\b/)
  assert.match(html, /data-ox-island="Price"/)
  assert.match(html, /data-ox-fw="vue"/)
  assert.match(html, /data-ox-pending="1"/)
  assert.doesNotMatch(html, /data-ox-chunk/, 'the foreign placeholder carries no chunk attr')
  assert.doesNotMatch(html, /data-ox-strategy/, 'the foreign placeholder carries no strategy attr')
  assert.doesNotMatch(html, /<span/, 'foreign children are not rendered by this adapter')
  const want = encodePropsB64({ a: 1 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(html, new RegExp(`data-ox-props="${want}"`))
  assert.match(html, /"><\/ox-island>$/, 'the placeholder is self-contained (no children)')
})

test('Island (solid) framework matching the page framework renders the local shape', async () => {
  const html = await renderToString(() =>
    Island({
      component: 'Counter',
      chunk: '/islands/counter.tsx',
      framework: 'solid',
      props: { initial: 3 },
      children: ssr('<span>3</span>'),
    }),
  )
  assert.match(html, /data-ox-chunk="\/islands\/counter\.tsx"/)
  assert.doesNotMatch(html, /data-ox-pending/, 'a local island is never pending')
  assert.match(html, /<span>3<\/span>/)
})

// The children pass through the same `ssrElement` builder the compiled
// pages use, so a snippet SSRNode and a plain node shape both render.
test('Island (solid) renders ssrElement-built children', async () => {
  const html = await renderToString(() =>
    Island({
      component: 'X',
      chunk: '/x.js',
      children: ssrElement('div', { class: 'wrap' }, ssr('inside'), false),
    }),
  )
  assert.match(html, /<div class="wrap">inside<\/div>/)
})
