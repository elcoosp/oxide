// @oxide/solid createRealAdapter factory tests (spec §7). Solid's SSR takes
// a zero-argument factory; the adapter wraps the caller's component with the
// (route,url,params,data) props so the shape is uniform across frameworks.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRealAdapter } from '../src/real-adapter.ts'
import { isSolidAvailable } from '../src/real-ssr.ts'
import { adapter as irAdapter } from '../src/index.ts'
import type { RenderInput } from '../../core/src/types.ts'

const available = await isSolidAvailable()
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
  assert.equal(a.name, 'solid')
  assert.deepEqual(a.extensions, ['.tsx'])
  assert.equal(a.jsxImportSource, '@solidjs/web')
  assert.deepEqual(a.capabilities, { suspense: 'native', islands: true, hmr: 'component' })
  const plugins = a.plugins({})
  assert.ok(plugins.length >= 1)
  assert.ok(plugins.every((p) => typeof p.name === 'string'))
})

test('factory: no component → falls back to the neutral IR renderer', async () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  const html = await drain(a.render(input()))
  const irHtml = await drain(irAdapter.render(input()))
  assert.equal(html, irHtml, 'IR path byte-identical to the default adapter')
  assert.ok(html.includes('data-solid'), 'solid root marker preserved')
})

test('factory: ssr=real + no component → OX-500 page (§10.2 step 5)', async () => {
  const a = createRealAdapter({ ssr: 'real', componentFor: () => undefined })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  assert.ok(html.includes('<!--oxide-status:500-->'), `500 marker: ${html}`)
  assert.ok(html.includes("data-ox-error='OX-500'"), `OX-500 div: ${html}`)
  assert.ok(html.includes('no component resolved for route routes/missing'), `diagnostic wording: ${html}`)
})

test('factory: real strategy renders a string-returning component', { skip: realSkip }, async () => {
  // Simplest portable component: returns a string primitive. Solid's SSR
  // accepts any value; this exercises the factory dispatch through the
  // shared helper without depending on the JSX runtime surface.
  const Greeting = (props: Record<string, unknown>) => {
    const data = (props.data ?? {}) as Record<string, unknown>
    return `Hello ${String(data.name ?? 'Solid')}`
  }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Greeting })
  const html = await drain(a.render(input({ data: { name: 'Ada' } })))
  assert.ok(html.includes('Hello Ada'), html)
  assert.ok(html.startsWith('<!doctype html>'), 'doctype first')
  assert.ok(html.includes('<title>Home · Atlas</title>'), 'layout titleTemplate applied')
  assert.ok(html.includes('data-solid'), 'solid root marker on container')
})

test('factory: real strategy maps a render throw to the core error boundary', { skip: realSkip }, async () => {
  const Boom = (): unknown => { throw new Error('solid component exploded') }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Boom })
  const html = await drain(a.render(input()))
  assert.ok(html.includes("data-ox-error='OX-500'"), html)
  assert.ok(html.includes('solid component exploded'), html)
})
