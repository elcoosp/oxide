// §17.1 / §7.1: the deferred-boundary swapper carries the request's CSP nonce.
//
// The swapper is an inline IIFE emitted with each `template[data-ox-c]`
// resolution. Under `strict-dynamic` an inline script without the policy's
// nonce is blocked — so before this, a CSP-configured deployment streamed
// resolutions that never ran: the placeholder stayed, the fallback never
// replaced. The pipeline computed a nonce for its own injected tags and
// never told the adapter.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { adapter } from '../src/index.ts'
import type { RenderInput } from '../../oxide/src/types.ts'

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/streaming',
  url: '/streaming',
  params: {},
  data: { comments: { __deferred: 1 }, meta: { __deferred: 2 } },
  deferred: { '1': [{ author: 'Grace', body: 'first' }], '2': 'meta' },
  head: { title: 'Streaming', meta: [], link: [], style: [], script: [] },
  ...overrides,
})

async function drain(stream: ReadableStream<Uint8Array>): Promise<string[]> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks
}

test('defer swapper: the resolution chunk carries the request nonce', async () => {
  const chunks = await drain(adapter.render(input({ nonce: 'n0nce-abc123' })))
  const later = chunks.slice(2).join('')
  assert.ok(later.includes("data-ox-c='0'"), `resolution chunk present:\n${later.slice(0, 400)}`)
  assert.ok(
    later.includes("nonce='n0nce-abc123'"),
    `the swapper script carries the nonce under strict-dynamic:\n${later.slice(0, 400)}`,
  )
})

test('defer swapper: no nonce configured means no attribute', async () => {
  const chunks = await drain(adapter.render(input()))
  const later = chunks.slice(2).join('')
  assert.ok(later.includes("data-ox-c='0'"), `resolution chunk present:\n${later.slice(0, 400)}`)
  assert.ok(
    !later.includes('nonce='),
    `no nonce attribute when none was supplied:\n${later.slice(0, 400)}`,
  )
})
