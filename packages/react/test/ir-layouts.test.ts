// §10.2 step 5 (P1-10): the IR **conformance** renderer must render the
// project's error component on a throw (it previously ignored
// `input.errorRoute` and always fell back to the generic OX-500 body).
//
// Layout nesting is deliberately NOT tested here: the IR's `PAGES` registry
// is the Atlas demo registry, which holds page components only, so a chain
// has nothing to resolve against. The real adapter owns the chain and
// `nested-layouts.test.ts` asserts it against `makeRealAdapter`.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { render } from '../src/render.ts'

async function collect(stream: ReadableStream<Uint8Array>): Promise<string> {
  const r = stream.getReader()
  const dec = new TextDecoder()
  let out = ''
  for (;;) {
    const { value, done } = await r.read()
    if (done) break
    out += dec.decode(value)
  }
  return out
}

test('IR: input.errorRoute renders the registry error component on a throw', async () => {
  // `routes/error`'s component throws. With `errorRoute: 'routes/not-found'`,
  // the error shell renders `NotFoundPage` (which carries `data-ox-not-found`)
  // instead of the generic OX-500 box.
  const html = await collect(
    render({
      route: 'routes/error',
      url: '/error',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
      errorRoute: 'routes/not-found',
    }),
  )
  assert.ok(
    html.includes('data-ox-not-found'),
    `the error component rendered: ${html.slice(0, 500)}`,
  )
  assert.ok(
    !html.includes("data-ox-error='OX-500'"),
    `the generic OX-500 boundary did NOT render: ${html.slice(0, 500)}`,
  )
})

test('IR: with no errorRoute the generic OX-500 boundary renders', async () => {
  const html = await collect(
    render({
      route: 'routes/error',
      url: '/error',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
    }),
  )
  assert.ok(
    html.includes("data-ox-error='OX-500'"),
    `the generic boundary rendered: ${html.slice(0, 500)}`,
  )
})

test('IR: an errorRoute that itself throws falls back to OX-500', async () => {
  // Point errorRoute at another throwing page (`routes/error` -> itself).
  const html = await collect(
    render({
      route: 'routes/error',
      url: '/error',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
      errorRoute: 'routes/error', // also throws
    }),
  )
  assert.ok(
    html.includes("data-ox-error='OX-500'"),
    `generic boundary after the error page threw: ${html.slice(0, 500)}`,
  )
})

test('IR: the default Atlas shell always wraps a page', async () => {
  const html = await collect(
    render({
      route: 'routes/index',
      url: '/',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
    }),
  )
  assert.ok(html.includes('Hello Atlas'), 'the page rendered')
  assert.ok(html.includes('<body>') && html.includes('</body></html>'), 'well-formed shell')
})
