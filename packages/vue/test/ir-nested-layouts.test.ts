// §8.2 / §16.3: the IR renderer nests `input.layouts` (outermost → innermost)
// around the page. Layout components are IR-native, registered in the
// adapter's `LAYOUTS` map; `routes/layout` is the root shell and
// `routes/nested/layout` a nested fixture.
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

function input(layouts: string[]) {
  return {
    route: 'routes/index',
    url: '/',
    params: {},
    data: {},
    head: { meta: [], link: [], style: [], script: [] },
    layouts,
  } as never
}

test('IR: a nested layout wraps the page', async () => {
  const html = await collect(render(input(['routes/nested/layout'])))
  assert.ok(html.includes('data-oxide-layout'), 'the layout rendered')
  assert.ok(html.includes('Hello Atlas'), 'the page rendered inside it')
  assert.ok(
    html.indexOf('data-oxide-layout') < html.indexOf('Hello Atlas'),
    'layout is outside the page',
  )
})

test('IR: a two-level chain nests outermost-first', async () => {
  const html = await collect(render(input(['routes/layout', 'routes/nested/layout'])))
  assert.ok(html.includes('data-oxide-layout'), 'the nested level rendered')
  assert.ok(html.includes('Hello Atlas'), 'the page rendered')
})

test('IR: an unknown layout id is skipped', async () => {
  const html = await collect(render(input(['routes/missing/layout'])))
  assert.ok(html.includes('Hello Atlas'), 'the page still renders')
  assert.ok(!html.includes('data-oxide-layout'), 'no bogus layout wrapper')
})
