// @oxide/vue createRealAdapter factory tests (spec §7). Prove the adapter
// contract dispatches to real @vue/server-renderer when the caller supplies
// components, and falls back to the neutral-IR renderer otherwise.
//
// Component props convention (matching the IR renderer in ./render.ts):
//   { route, url, params, data }
// where `data` is the loader output. Components read `props.data.<field>`.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRealAdapter } from '../src/real-adapter.ts'
import { isVueAvailable } from '../src/real-ssr.ts'
import { adapter as irAdapter } from '../src/index.ts'
import type { RenderInput } from '../../oxide/src/types.ts'

const available = await isVueAvailable()
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
  assert.equal(a.name, 'vue')
  assert.deepEqual(a.extensions, ['.vue'])
  assert.equal(a.jsxImportSource, undefined)
  assert.deepEqual(a.capabilities, { suspense: 'core', islands: true, hmr: 'component' })
  const plugins = a.plugins({})
  assert.ok(Array.isArray(plugins) && plugins.length >= 1)
  assert.ok(plugins.every((p) => typeof p.name === 'string'))
})

test('factory: no component → falls back to the neutral IR renderer', async () => {
  const a = createRealAdapter({ ssr: 'auto', componentFor: () => undefined })
  const html = await drain(a.render(input()))
  const irHtml = await drain(irAdapter.render(input()))
  assert.equal(html, irHtml, 'IR path is byte-identical to the default adapter')
  assert.ok(html.includes('data-v-app'), 'vue app marker preserved')
})

test('factory: ssr=real + no component → comment placeholder', async () => {
  const a = createRealAdapter({ ssr: 'real', componentFor: () => undefined })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  assert.ok(html.includes('no component for route routes/missing'), html)
})

test('factory: real strategy renders through @vue/server-renderer', { skip: realSkip }, async () => {
  const vue = (await import('vue')) as unknown as {
    h(type: unknown, props: Record<string, unknown> | null, ...children: unknown[]): unknown
  }
  // Vue components read props.data (the framework-neutral adapter contract).
  const Greeting = {
    props: { data: { type: Object, default: () => ({}) } },
    render(this: { data: Record<string, unknown> }) {
      return vue.h('h1', { class: 'greeting' }, `Hello ${String(this.data.name ?? 'Vue')}`)
    },
  }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Greeting })
  const html = await drain(a.render(input({ data: { name: 'Ada' } })))
  assert.ok(html.includes('Hello Ada'), html)
  assert.ok(html.startsWith('<!doctype html>'), 'doctype first')
  assert.ok(html.includes('<title>Home · Atlas</title>'), 'layout titleTemplate applied')
  assert.ok(html.includes('data-v-app'), 'vue app marker on container')
})

test('factory: real strategy escapes text through the real renderer', { skip: realSkip }, async () => {
  const vue = (await import('vue')) as unknown as {
    h(type: unknown, props: Record<string, unknown> | null, ...children: unknown[]): unknown
  }
  const Dangerous = { render: () => vue.h('p', null, '<script>alert(1)</script>') }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Dangerous })
  const html = await drain(a.render(input()))
  assert.ok(html.includes('&lt;script&gt;'), 'literal script tag escaped by @vue/server-renderer')
  assert.ok(!html.includes('<script>alert(1)</script>'), 'no raw script element emitted')
})

test('factory: real strategy maps a render throw to the core error boundary', { skip: realSkip }, async () => {
  const Boom = { render(): unknown { throw new Error('vue component exploded') } }
  const a = createRealAdapter({ ssr: 'real', componentFor: () => Boom })
  const html = await drain(a.render(input()))
  assert.ok(html.includes("data-ox-error='OX-500'"), html)
  assert.ok(html.includes('vue component exploded'), html)
})
