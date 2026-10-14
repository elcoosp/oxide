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
import assert from 'node:assert/strict'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Island } from '../src/island.ts'
import { toBase64 } from '../src/ir.ts'

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
  assert.match(html, new RegExp(`data-ox-props="${toBase64('{"initial":3}')}"`))
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
