// §22 / D20 — the conformance suite. One reference app (Atlas), four adapters,
// the SAME assertions. This is the spec's release gate: "A release is blocked
// if any adapter regresses."
//
// Every assertion here runs against all four IR renderers, which are the
// framework-neutral *conformance* renderers (the real adapters delegate their
// component rendering to React/Vue/Svelte/Solid; the markup contract below is
// what those real renderers must also satisfy). The point is that the wire
// shapes are byte-identical across frameworks — that is what makes the core's
// boundary protocol, island protocol and error model work uniformly.
//
// Adding an adapter: add it to `ADAPTERS`; every test then covers it.

import { test } from 'node:test'
import assert from 'node:assert/strict'

import * as reactRender from '../../react/src/render.ts'
import * as vueRender from '../../vue/src/render.ts'
import * as svelteRender from '../../svelte/src/render.ts'
import * as solidRender from '../../solid/src/render.ts'

interface Adapter {
  name: string
  render: (input: unknown) => ReadableStream<Uint8Array>
  renderShellToString?: (input: unknown) => string
}

const ADAPTERS: Adapter[] = [
  { name: 'react', render: reactRender.render as never, renderShellToString: reactRender.renderShellToString as never },
  { name: 'vue', render: vueRender.render as never, renderShellToString: vueRender.renderShellToString as never },
  { name: 'svelte', render: svelteRender.render as never, renderShellToString: svelteRender.renderShellToString as never },
  { name: 'solid', render: solidRender.render as never, renderShellToString: solidRender.renderShellToString as never },
]

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

function input(route: string, data: Record<string, unknown> = {}): unknown {
  return {
    route,
    url: `/${route}`,
    params: {},
    data,
    head: { meta: [], link: [], style: [], script: [] },
  }
}

// ---------------------------------------------------------------- SSR output

test('conformance: every adapter emits a well-formed document shell', async () => {
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/index')))
    assert.ok(html.startsWith('<!doctype html>'), `${a.name}: starts with doctype`)
    assert.ok(html.includes('<html'), `${a.name}: has <html`)
    assert.ok(html.includes('<body>'), `${a.name}: has <body>`)
    assert.ok(html.includes('</body></html>'), `${a.name}: closes cleanly`)
    assert.ok(html.includes('id="root"'), `${a.name}: has the #root host`)
  }
})

test('conformance: every adapter renders the page component', async () => {
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/index')))
    assert.ok(
      html.includes('Hello Atlas'),
      `${a.name}: the home page body rendered: ${html.slice(0, 300)}`,
    )
  }
})

// ---------------------------------------------------------------- error boundary (§10.2 step 5)

test('conformance: a throwing page maps to the OX-500 error boundary', async () => {
  for (const a of ADAPTERS) {
    // `routes/error`'s component throws by design (throwsAtRender).
    const html = await collect(a.render(input('routes/error')))
    assert.ok(
      html.includes("data-ox-error='OX-500'"),
      `${a.name}: the error boundary rendered: ${html.slice(0, 400)}`,
    )
  }
})

test('conformance: the error boundary is uniform across adapters', async () => {
  const outputs: Record<string, string> = {}
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/error')))
    const m = /<div class='ox-error' data-ox-error='OX-500'>/.exec(html)
    assert.ok(m, `${a.name}: the exact boundary marker is present`)
    outputs[a.name] = m![0]
  }
  const first = outputs[ADAPTERS[0].name]
  for (const a of ADAPTERS) {
    assert.equal(outputs[a.name], first, `${a.name}: same boundary markup as ${ADAPTERS[0].name}`)
  }
})

// ---------------------------------------------------------------- deferred boundary (§7.1)

test('conformance: a deferred value emits the boundary placeholder shape', async () => {
  for (const a of ADAPTERS) {
    // `routes/streaming` renders a deferred boundary.
    const html = await collect(a.render(input('routes/streaming')))
    assert.ok(
      html.includes('data-ox-b='),
      `${a.name}: a deferred boundary placeholder rendered: ${html.slice(0, 400)}`,
    )
  }
})

// ---------------------------------------------------------------- islands (§10.3)

test('conformance: an island renders the ox-island host shape', async () => {
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/islands')))
    assert.ok(
      html.includes('<ox-island'),
      `${a.name}: an <ox-island> host rendered: ${html.slice(0, 400)}`,
    )
    assert.ok(
      html.includes('data-ox-chunk='),
      `${a.name}: the island carries a chunk reference`,
    )
    assert.ok(
      html.includes('data-ox-props='),
      `${a.name}: the island carries ODF-encoded props`,
    )
  }
})

// ---------------------------------------------------------------- hydration bootstrap (§9.5)

test('conformance: an ODF payload emits the application/oxide-data script', async () => {
  const odf = new TextEncoder().encode('ODF1')
  for (const a of ADAPTERS) {
    const html = await collect(
      a.render({
        ...(input('routes/index') as Record<string, unknown>),
        odf,
      } as never),
    )
    assert.ok(
      html.includes("type='application/oxide-data'") || html.includes('type="application/oxide-data"'),
      `${a.name}: the ODF bootstrap script rendered`,
    )
    assert.ok(html.includes('window.__oxide='), `${a.name}: the router bootstrap emitted`)
  }
})

// ---------------------------------------------------------------- head merge (§16.3)

test('conformance: the page head merges into the document head', async () => {
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/head')))
    const headEnd = html.indexOf('</head>')
    assert.ok(headEnd > 0, `${a.name}: a <head> exists`)
    assert.ok(
      html.slice(0, headEnd).includes('page-head-extracted'),
      `${a.name}: the page's own <meta> merged into <head>`,
    )
  }
})

// ---------------------------------------------------------------- not-found (§10.2 step 5)

test('conformance: an unknown route renders the not-found boundary', async () => {
  for (const a of ADAPTERS) {
    const html = await collect(a.render(input('routes/does-not-exist')))
    assert.ok(
      html.includes('data-ox-not-found') || html.includes('This Atlas route does not exist'),
      `${a.name}: the not-found page rendered: ${html.slice(0, 400)}`,
    )
  }
})
