// `<Island>` (vue) — the Vue adapter's island wrapper (§V2-F01): the twin of
// `packages/react/src/island.ts`, emitting the exact `<ox-island>` shape the
// render crate emits and the client boot script scans for, and the
// self-contained pending placeholder for a FOREIGN island (the worker's
// `island:render` op + the core `resolveForeign` transform resolve it with
// the island's own adapter).
//
// The test renders through the real `@vue/server-renderer` so the
// attribute-emission path is the one production uses.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { h, type VNode } from 'vue'
import { renderToString } from '@vue/server-renderer'
import { encodePropsB64 } from '../../core/src/odf-encode.ts'
import { Island } from '../src/island.ts'

function render(node: VNode): Promise<string> {
  return renderToString(node)
}

test('Island (vue) emits the render-crate ox-island shape', async () => {
  const html = await render(
    h(
      Island,
      {
        component: 'Counter',
        chunk: '/islands/counter.js',
        strategy: 'load',
        props: { initial: 3 },
      },
      () => h('button', [h('span', '3')]),
    ),
  )
  assert.match(html, /^<ox-island\b/)
  assert.match(html, /data-ox-island="Counter"/)
  assert.match(html, /data-ox-chunk="\/islands\/counter\.js"/)
  assert.match(html, /data-ox-strategy="load"/)
  // PERF-01: props ride ODF now; compare against the encoder's output.
  const want = encodePropsB64({ initial: 3 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(html, new RegExp(`data-ox-props="${want}"`))
  assert.match(html, /<button><span>3<\/span><\/button>/)
})

test('Island (vue) default strategy is "load"', async () => {
  const html = await render(h(Island, { component: 'X', chunk: '/x.js' }, () => h('span', 'x')))
  assert.match(html, /data-ox-strategy="load"/)
})

test('Island (vue) media strategy serializes as media(...)', async () => {
  const html = await render(
    h(
      Island,
      { component: 'X', chunk: '/x.js', strategy: { media: '(min-width: 800px)' } },
      () => h('span', 'x'),
    ),
  )
  assert.match(html, /data-ox-strategy="media\(min-width: 800px\)"/)
})

test('Island (vue) emits the pending placeholder for a foreign framework island', async () => {
  const html = await render(
    h(
      Island,
      {
        component: 'Ticker',
        chunk: '/islands/Ticker.tsx',
        framework: 'solid',
        props: { tick: 1 },
      },
      () => h('span', 'ignored'),
    ),
  )
  assert.match(html, /^<ox-island\b/)
  assert.match(html, /data-ox-island="Ticker"/)
  assert.match(html, /data-ox-fw="solid"/)
  assert.match(html, /data-ox-pending="1"/)
  assert.doesNotMatch(html, /data-ox-chunk/, 'the foreign placeholder carries no chunk attr')
  assert.doesNotMatch(html, /data-ox-strategy/, 'the foreign placeholder carries no strategy attr')
  assert.doesNotMatch(html, /<span/, 'foreign children are not rendered by this adapter')
  const want = encodePropsB64({ tick: 1 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(html, new RegExp(`data-ox-props="${want}"`))
  assert.match(html, /"><\/ox-island>$/, 'the placeholder is self-contained (no children)')
})

test('Island (vue) framework matching the page framework renders the local shape', async () => {
  const html = await render(
    h(
      Island,
      { component: 'Counter', chunk: '/islands/counter.js', framework: 'vue', props: { initial: 3 } },
      () => h('span', '3'),
    ),
  )
  assert.match(html, /data-ox-chunk="\/islands\/counter\.js"/)
  assert.doesNotMatch(html, /data-ox-pending/, 'a local island is never pending')
  assert.match(html, /<span>3<\/span>/)
})
