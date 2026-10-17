// §10.2 step 5: when a page throws during SSR and `input.errorRoute` points
// at an `error.tsx` component, the adapter renders that component with the
// thrown error rather than the generic OX-500 text. When errorRoute is
// absent (or its component throws too), the generic OX-500 body stands.
//
// The critical case is the `props` reference: the previous patch declared
// `props` inside the try block and referenced it from catch — a
// ReferenceError at runtime. Node's type-stripping does not type-check, so
// only a live test catches it. This file is the regression guard.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { makeRealAdapter } from '../src/server/real-ssr.ts'

async function collect(stream: ReadableStream<Uint8Array>): Promise<string> {
  const reader = stream.getReader()
  const dec = new TextDecoder()
  let out = ''
  for (;;) {
    const { value, done } = await reader.read()
    if (done) break
    out += dec.decode(value)
  }
  return out
}

function adapterWith(components: Record<string, unknown>) {
  return makeRealAdapter({
    name: 'fake',
    extensions: ['.tsx'],
    capabilities: { suspense: 'native', islands: true, hmr: 'refresh' },
    plugins: () => [],
    irRender: () => new ReadableStream<Uint8Array>(),
    buildHead: () => ({ meta: [], link: [], style: [], script: [] }),
    isAvailable: async () => true,
    renderBody: async (Component, props) => {
      // A `Tag` object renders `<Tag ...>`; a `Throw` object throws.
      const c = Component as { __kind?: string; __tag?: string }
      if (c && c.__kind === 'throw') {
        throw new Error('page exploded')
      }
      const tag = (c && c.__tag) || 'Unknown'
      const errMsg = (props.error as { message?: string } | undefined)?.message
      return `<${tag}${errMsg ? ` error="${errMsg}"` : ''}></${tag}>`
    },
    rootAttrs: ' id="root"',
    hydrate: (() => () => {}) as never,
    mount: (() => () => {}) as never,
    update: (() => {}) as never,
    opts: {
      ssr: 'real',
      componentFor: (route: string) => components[route],
    },
  })
}

test('error boundary: errorRoute is rendered with the thrown message', async () => {
  const adapter = adapterWith({
    'routes/index': { __kind: 'throw' },
    'routes/error': { __tag: 'ErrorPage' },
  })
  const html = await collect(
    adapter.render({
      route: 'routes/index',
      url: '/',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
      errorRoute: 'routes/error',
    } as never),
  )
  assert.match(html, /<ErrorPage error="page exploded"><\/ErrorPage>/, 'error.tsx rendered with message')
})

test('error boundary: without errorRoute the generic OX-500 is emitted', async () => {
  const adapter = adapterWith({
    'routes/index': { __kind: 'throw' },
  })
  const html = await collect(
    adapter.render({
      route: 'routes/index',
      url: '/',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
    } as never),
  )
  assert.match(html, /data-ox-error='OX-500'/)
  assert.match(html, /page exploded/)
})

test('error boundary: a throwing error.tsx falls back to OX-500', async () => {
  const adapter = adapterWith({
    'routes/index': { __kind: 'throw' },
    'routes/error': { __kind: 'throw' },
  })
  const html = await collect(
    adapter.render({
      route: 'routes/index',
      url: '/',
      params: {},
      data: {},
      head: { meta: [], link: [], style: [], script: [] },
      errorRoute: 'routes/error',
    } as never),
  )
  assert.match(html, /data-ox-error='OX-500'/, 'generic page emitted when error.tsx also throws')
})

test('error boundary: props stays in scope in catch (regression)', async () => {
  // Specifically guard the ReferenceError that would have fired if `props`
  // remained inside the try block.
  const adapter = adapterWith({
    'routes/index': { __kind: 'throw' },
    'routes/error': { __tag: 'ErrorPage' },
  })
  // If `props` went out of scope, this would throw instead of resolving.
  const html = await collect(
    adapter.render({
      route: 'routes/index',
      url: '/',
      params: { slug: 'x' },
      data: { hello: 'world' },
      head: { meta: [], link: [], style: [], script: [] },
      errorRoute: 'routes/error',
    } as never),
  )
  assert.ok(html.includes('<ErrorPage'), 'catch rendered without a ReferenceError')
})
