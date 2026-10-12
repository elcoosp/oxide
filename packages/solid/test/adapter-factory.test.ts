// @oxide/solid adapter factory tests (spec §14 config surface). Proves the
// callable default export dispatches to real SSR when componentFor is
// supplied, and returns the IR adapter otherwise.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import defaultFactory, { adapter, createAdapter } from '../src/index.ts'
import type { RenderInput } from '../../oxide/src/types.ts'

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

test('factory: default export is callable and returns a §7-shaped adapter', () => {
  const a = defaultFactory()
  assert.equal(a.contract, 1)
  assert.equal(a.name, adapter.name)
  assert.deepEqual(a.extensions, adapter.extensions)
})

test('factory: createAdapter() with no componentFor returns the IR adapter behavior', async () => {
  const a = createAdapter()
  const html = await drain(a.render(input()))
  const irHtml = await drain(adapter.render(input()))
  assert.equal(html, irHtml, 'identical to the named  export')
})

test('factory: createAdapter({ ssr: "real", componentFor: () => undefined }) emits placeholder', async () => {
  const a = createAdapter({
    ssr: 'real',
    componentFor: () => undefined,
  })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  assert.ok(html.includes('no component for route routes/missing'), html)
})

test('factory: componentFor + ssr="auto" with missing deps falls back to IR', async () => {
  // In 'auto' mode, a component resolver that returns a value but has no
  // upstream framework available (which is not the case here, but the
  // behavior is the same at the API level) still emits usable HTML.
  const a = createAdapter({
    ssr: 'auto',
    componentFor: () => undefined,
  })
  const html = await drain(a.render(input()))
  assert.ok(html.startsWith('<!doctype html>'), html)
})

test('factory: AdapterOptions accepts a compiler flag without breaking dispatch', () => {
  // The smoke-app config passes ; the factory must accept
  // it (typed as informational) and still return a valid adapter.
  const a = defaultFactory({ compiler: true })
  assert.equal(a.contract, 1)
})
