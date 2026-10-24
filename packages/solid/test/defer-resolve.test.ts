// §7.1 / §9.2: a loader's `event.defer()` value reaches the component.
//
// The loader returns immediately with a `{ __deferred: id }` placeholder in
// its data; the pipeline collects the settle stream and hands the resolved
// map to the adapter as `input.deferred`. Before that was threaded through,
// a component read `props.data.<key>` as the placeholder object and rendered
// it — `defer()` was broken on the server render path on every adapter.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createRealAdapter } from '../src/real-adapter.ts'
import type { RenderInput } from '../../core/src/types.ts'

const Component = (props: { data?: Record<string, unknown> }) =>
  `RENDERED:${JSON.stringify(props.data ?? null)}`

const adapterWith = () =>
  createRealAdapter({
    ssr: 'real',
    componentFor: (route: string) => (route === 'routes/probe' ? Component : undefined),
  })

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/probe',
  url: '/probe',
  params: {},
  data: {},
  head: { title: 'Probe', meta: [], link: [], style: [], script: [] },
  ...overrides,
})

async function render(overrides: Partial<RenderInput>): Promise<string> {
  const stream = adapterWith().render(input(overrides))
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  // The framework HTML-escapes text nodes, so a component printing JSON
  // renders `&quot;`. Decode before asserting on the payload.
  return chunks
    .join('')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&amp;/g, '&')
}

test('defer: a placeholder in data is replaced by the resolved value', async () => {
  const html = await render({
    data: { comments: { __deferred: 1 }, title: 'Post' },
    deferred: { '1': ['first', 'second'] },
  })
  assert.ok(html.includes('"comments":["first","second"]'), `resolved:\n${html.slice(0, 400)}`)
  assert.ok(!html.includes('__deferred'), `no placeholder survives:\n${html.slice(0, 400)}`)
})

test('defer: the placeholder is untouched when the settle never arrived', async () => {
  // A rejected or missing settle leaves the placeholder: a visibly wrong
  // object is a better signal than silently becoming `undefined`.
  const html = await render({ data: { comments: { __deferred: 7 } } })
  assert.ok(html.includes('__deferred'), `placeholder preserved:\n${html.slice(0, 400)}`)
})

test('defer: nesting is walked (arrays, objects, depth)', async () => {
  const html = await render({
    data: {
      nested: { deep: [{ __deferred: 2 }] },
      list: [{ __deferred: 3 }, 'plain'],
    },
    deferred: { '2': 'inner', '3': 'listed' },
  })
  assert.ok(html.includes('"deep":["inner"]'), `nested:\n${html.slice(0, 400)}`)
  assert.ok(html.includes('["listed","plain"]'), `list:\n${html.slice(0, 400)}`)
})

test('defer: no deferred map is a no-op, not a crash', async () => {
  const html = await render({ data: { plain: 'value' } })
  assert.ok(html.includes('"plain":"value"'), `plain data:\n${html.slice(0, 400)}`)
})
