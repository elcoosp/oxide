// `<Island>` renders the exact `<ox-island>` shape the render crate emits
// and the client boot script scans for. The test uses the real
// `react-dom/server` renderer so the JSX-to-HTML path is the same one
// production uses.
//
// The test is `.ts` and builds elements with `createElement` — the package
// test runner is Node's built-in `--test`, which strips types but does not
// transform `.tsx`. The component itself also uses `createElement` (see
// ../src/island.ts), so no JSX is required anywhere in this file.

import { test } from 'node:test'
import { encodePropsB64 } from '../../core/src/odf-encode.ts'
import assert from 'node:assert/strict'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Island } from '../src/island.ts'
import { toBase64 } from '../../core/src/bytes.ts'

function render(node: ReactNode): string {
  return renderToStaticMarkup(node as Parameters<typeof renderToStaticMarkup>[0])
}

test('Island emits the render-crate ox-island shape', () => {
  const html = render(
    createElement(
      Island,
      {
        component: 'Counter',
        chunk: '/islands/counter.js',
        strategy: 'load',
        props: { initial: 3 },
      },
      createElement('button', null, createElement('span', null, '3')),
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

test('Island default strategy is "load"', () => {
  const html = render(
    createElement(
      Island,
      { component: 'X', chunk: '/x.js' },
      createElement('span', null, 'x'),
    ),
  )
  assert.match(html, /data-ox-strategy="load"/)
})

test('Island media strategy serializes as media(...)', () => {
  const html = render(
    createElement(
      Island,
      { component: 'X', chunk: '/x.js', strategy: { media: '(min-width: 800px)' } },
      createElement('span', null, 'x'),
    ),
  )
  assert.match(html, /data-ox-strategy="media\(min-width: 800px\)"/)
})

test('Island escapes props into base64, never raw markup', () => {
  const html = render(
    createElement(
      Island,
      { component: 'X', chunk: '/x.js', props: { s: `"</script><script>alert(1)</script>` } },
      createElement('span', null, 'x'),
    ),
  )
  assert.ok(!html.includes('</script>'), 'no script tag should appear in output')
  assert.match(html, /data-ox-props="[A-Za-z0-9+/=]+"/)
})

// §V2-F01 item 4: a FOREIGN island (the `framework` prop set to another
// framework) emits the self-contained pending placeholder the worker's
// `island:render` op + the core `resolveForeign` transform resolve — no
// chunk/strategy attrs (the foreign chunk is the island's own), and the
// children are NOT rendered (a foreign framework's children cannot be SSR'd
// by this adapter).
test('Island emits the pending placeholder for a foreign framework island', () => {
  const html = render(
    createElement(Island, {
      component: 'Price',
      chunk: '/islands/Price.vue',
      framework: 'vue',
      props: { a: 1 },
      children: createElement('span', null, 'ignored'),
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

test('Island framework matching the page framework renders the local shape', () => {
  const html = render(
    createElement(
      Island,
      { component: 'Counter', chunk: '/islands/counter.js', framework: 'react', props: { initial: 3 } },
      createElement('span', null, '3'),
    ),
  )
  assert.match(html, /data-ox-chunk="\/islands\/counter\.js"/)
  assert.doesNotMatch(html, /data-ox-pending/, 'a local island is never pending')
  assert.match(html, /<span>3<\/span>/)
})
