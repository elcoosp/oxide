// @oxide/react createRealAdapter factory tests (spec §7). Prove the adapter
// contract can dispatch to real react-dom/server when the caller supplies
// components, and falls back to the neutral-IR renderer otherwise.
//
// Component props convention (matching the IR renderer in ./render.ts and the
// framework-neutral adapter contract): a component receives
//   { route, url, params, data }
// where `data` is the loader output (RenderInput.data). Pages read
// `props.data.<field>`; the same shape flows through every adapter.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRealAdapter } from '../src/real-adapter.ts'
import { isReactAvailable } from '../src/real-ssr.ts'
import { adapter as irAdapter } from '../src/index.ts'
import type { RenderInput } from '../../core/src/types.ts'

const available = await isReactAvailable()
const realSkip = !available

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/index',
  url: '/',
  params: {},
  data: {},
  head: { title: 'Home', meta: [{ name: 'description', content: 'x' }], link: [], style: [], script: [] },
  ...overrides,
})

async function drain(stream: ReadableStream<Uint8Array>): Promise<string> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks.join('')
}

test('factory: createRealAdapter returns a §7-shaped FrameworkAdapter', () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  assert.equal(a.contract, 1)
  assert.equal(a.name, 'react')
  assert.deepEqual(a.extensions, ['.tsx'])
  assert.equal(a.jsxImportSource, 'react')
  assert.deepEqual(a.capabilities, { suspense: 'native', islands: true, hmr: 'refresh' })
  const plugins = a.plugins({})
  assert.ok(Array.isArray(plugins) && plugins.length >= 1)
  assert.ok(plugins.every((p) => typeof p.name === 'string'))
})

test('factory: no component → falls back to the neutral IR renderer', async () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  const html = await drain(a.render(input()))
  const irHtml = await drain(irAdapter.render(input()))
  assert.equal(html, irHtml, 'IR path is byte-identical to the default adapter')
  assert.ok(html.includes('data-reactroot'), 'react root marker preserved')
})

test('factory: ssr=real + no component → OX-500 page (§10.2 step 5)', async () => {
  const a = createRealAdapter({ ssr: 'real', componentFor: () => undefined })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  // §6.4 / §10.2 step 5: the miss is a real HTTP 500 (the marker is
  // the adapter's status contract). Not a silent comment.
  assert.ok(html.includes('<!--oxide-status:500-->'), `500 marker: ${html}`)
  assert.ok(html.includes("data-ox-error='OX-500'"), `OX-500 div: ${html}`)
  assert.ok(html.includes('no component resolved for route routes/missing'), `diagnostic wording: ${html}`)
})

test('factory: real strategy renders through react-dom/server', { skip: realSkip }, async () => {
  const React = await import('react')
  // Component reads from props.data — the framework-neutral adapter contract.
  const Greeting = (props: Record<string, unknown>) => {
    const data = (props.data ?? {}) as Record<string, unknown>
    return React.createElement('h1', { className: 'greeting' }, `Hello ${String(data.name ?? 'React')}`)
  }
  const a = createRealAdapter({
    ssr: 'real',
    componentFor: () => Greeting,
  })
  const html = await drain(a.render(input({ data: { name: 'Ada' } })))
  assert.ok(html.includes('<h1 class="greeting">Hello Ada</h1>'), html)
  assert.ok(html.startsWith('<!doctype html>'), 'doctype first')
  // LAYOUT_HEAD carries titleTemplate '%s · Atlas'; the route head title
  // 'Home' is templated to 'Home · Atlas' — same as the IR adapter's own
  // test asserts, confirming the real-adapter's head merge matches.
  assert.ok(html.includes('<title>Home · Atlas</title>'), 'head emitted from IR model (layout titleTemplate applied)')
  assert.ok(html.includes('data-reactroot'), 'react root marker on container')
  assert.ok(!html.includes('data-ox-b'), 'no IR boundary artifacts in real path')
})

test('factory: route + params reach the component', { skip: realSkip }, async () => {
  const React = await import('react')
  const Post = (props: Record<string, unknown>) => {
    const params = (props.params ?? {}) as Record<string, unknown>
    return React.createElement('h1', null, `Post: ${String(params.slug ?? '')}`)
  }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Post })
  const html = await drain(a.render(input({
    route: 'routes/blog/[slug]', url: '/blog/hello', params: { slug: 'hello' },
  })))
  assert.ok(html.includes('Post: hello'), html)
})

test('factory: real strategy escapes text through the real renderer', { skip: realSkip }, async () => {
  const React = await import('react')
  const Dangerous = () => React.createElement('p', null, '<script>alert(1)</script>')
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Dangerous })
  const html = await drain(a.render(input()))
  assert.ok(html.includes('&lt;script&gt;'), 'literal script tag escaped by react-dom/server')
  assert.ok(!html.includes('<script>alert(1)</script>'), 'no raw script element emitted')
})

test('factory: real strategy maps a render throw to the core error boundary', { skip: realSkip }, async () => {
  const Boom = () => { throw new Error('react component exploded') }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Boom })
  const html = await drain(a.render(input()))
  assert.ok(html.includes("data-ox-error='OX-500'"), html)
  assert.ok(html.includes('react component exploded'), html)
})
