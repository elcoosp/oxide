// §8.2 / §16.3 nested layouts. `makeRealAdapter` receives the router's
// layout chain on `input.layouts` (outermost → innermost) and must nest the
// page inside them: L1(L2(L3(page))). The `layoutFor(route)` fallback (a
// single layer) must still work for configs that predate `input.layouts`.
//
// The test uses a scripted `renderBody` that renders each component as
// `<Tag>{children}</Tag>` so we can assert the exact nesting order in the
// emitted HTML without React (or any framework) installed.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { makeRealAdapter } from '../src/server/real-ssr.ts'

interface Marker { __tag: string }

function tag(name: string): Marker {
  return { __tag: name }
}

/** A `renderBody` that walks the {Component, props} descriptor tree. */
async function scriptedRenderBody(Component: unknown, props: Record<string, unknown>): Promise<string> {
  const m = Component as Marker
  const name = (m && typeof m === 'object' && '__tag' in m) ? m.__tag : String(Component)
  const children = props.children as { Component: unknown; props: Record<string, unknown> } | undefined
  const inner = children
    ? await scriptedRenderBody(children.Component, children.props)
    : (typeof props.__leaf === 'string' ? props.__leaf : '')
  return `<${name}>${inner}</${name}>`
}

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

function adapterWith(components: Record<string, unknown>, layoutFor?: (r: string) => unknown) {
  return makeRealAdapter({
    name: 'fake',
    extensions: ['.tsx'],
    capabilities: { suspense: 'native', islands: true, hmr: 'refresh' },
    plugins: () => [],
    irRender: () => new ReadableStream<Uint8Array>(),
    buildHead: () => ({ meta: [], link: [], style: [], script: [] }),
    isAvailable: async () => true,
    renderBody: scriptedRenderBody as never,
    rootAttrs: ' id="root"',
    hydrate: (() => () => {}) as never,
    mount: (() => () => {}) as never,
    update: (() => {}) as never,
    opts: {
      ssr: 'real',
      componentFor: (route: string) => components[route],
      ...(layoutFor ? { layoutFor } : {}),
    },
  })
}

function inputFor(route: string, layouts: string[] | undefined) {
  return {
    route,
    url: '/' + route,
    params: {},
    data: {},
    head: { meta: [], link: [], style: [], script: [] },
    ...(layouts ? { layouts } : {}),
  }
}

test('layouts: single-layer chain wraps the page (outermost → innermost)', async () => {
  const adapter = adapterWith({
    'routes/blog/[slug]': tag('Page'),
    'routes/blog/layout': tag('BlogLayout'),
  })
  const html = await collect(adapter.render(inputFor('routes/blog/[slug]', ['routes/blog/layout']) as never))
  assert.match(html, /<BlogLayout><Page><\/Page><\/BlogLayout>/, 'page nested inside one layout')
})

test('layouts: three-layer chain nests outermost-first', async () => {
  const adapter = adapterWith({
    'routes/blog/[slug]': tag('Page'),
    'routes/blog/layout': tag('BlogLayout'),
    'routes/layout': tag('RootLayout'),
  })
  const html = await collect(
    adapter.render(inputFor('routes/blog/[slug]', ['routes/layout', 'routes/blog/layout']) as never),
  )
  // Outermost (root) wraps the blog layout, which wraps the page.
  assert.match(html, /<RootLayout><BlogLayout><Page><\/Page><\/BlogLayout><\/RootLayout>/)
})

test('layouts: an unresolved layout id is skipped in `auto`, the rest still nest', async () => {
  const adapter = makeRealAdapter({
    name: 'fake',
    extensions: ['.tsx'],
    capabilities: { suspense: 'native', islands: true, hmr: 'refresh' },
    plugins: () => [],
    irRender: () => new ReadableStream<Uint8Array>(),
    buildHead: () => ({ meta: [], link: [], style: [], script: [] }),
    isAvailable: async () => true,
    renderBody: scriptedRenderBody as never,
    rootAttrs: ' id="root"',
    hydrate: (() => () => {}) as never,
    mount: (() => () => {}) as never,
    update: (() => {}) as never,
    opts: {
      ssr: 'auto',
      componentFor: (route: string) =>
        route === 'routes/blog/[slug]' ? tag('Page') : route === 'routes/layout' ? tag('RootLayout') : undefined,
    },
  })
  const html = await collect(
    adapter.render(
      inputFor('routes/blog/[slug]', ['routes/layout', 'routes/blog/layout']) as never,
    ),
  )
  assert.match(html, /<RootLayout><Page><\/Page><\/RootLayout>/, 'root layout still wraps the page')
})

test('layouts: layoutFor(route) fallback still wraps when input.layouts is absent', async () => {
  const adapter = adapterWith(
    { 'routes/blog/[slug]': tag('Page') },
    () => tag('LegacyLayout'),
  )
  const html = await collect(adapter.render(inputFor('routes/blog/[slug]', undefined) as never))
  assert.match(html, /<LegacyLayout><Page><\/Page><\/LegacyLayout>/, 'single-layer layoutFor still works')
})

test('layouts: no layouts in the chain renders the page directly', async () => {
  const adapter = adapterWith({ 'routes/index': tag('Page') })
  const html = await collect(adapter.render(inputFor('routes/index', []) as never))
  assert.match(html, /<div id="root"><Page><\/Page><\/div>/)
})
