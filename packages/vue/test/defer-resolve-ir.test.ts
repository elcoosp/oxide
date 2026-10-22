// §7.1 / §9.2: the IR renderer resolves `defer()` placeholders too.
//
// `defer-resolve.test.ts` covers the real adapter. The IR renderer (the
// named `adapter` export) has its own `defer(...)` boundary node, and it
// resolved the wrong thing: the boundary's value is a `{ __deferred: id }`
// placeholder, not a promise, so `deferPromise()` returned the object and
// the render callback ran against it — `.map is not a function` on the
// Atlas streaming fixture.
//
// These drive the conformance route (`routes/streaming`, the only route the
// IR registry knows) and assert the resolution chunk carries the values.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { adapter } from '../src/index.ts'
import type { RenderInput } from '../../oxide/src/types.ts'

const COMMENTS = [
  { author: 'Grace', body: 'first' },
  { author: 'Alan', body: 'second' },
]

const input = (overrides: Partial<RenderInput> = {}): RenderInput => ({
  route: 'routes/streaming',
  url: '/streaming',
  params: {},
  data: {},
  head: { title: 'Streaming', meta: [], link: [], style: [], script: [] },
  ...overrides,
})

async function drain(stream: ReadableStream<Uint8Array>): Promise<string[]> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks
}

test('IR defer: the resolution chunk carries the resolved value', async () => {
  const chunks = await drain(
    adapter.render(
      input({
        data: { comments: { __deferred: 1 }, meta: { __deferred: 2 } },
        deferred: { '1': COMMENTS, '2': 'resolved-meta' },
      }),
    ),
  )
  const later = chunks.slice(2).join('')
  assert.ok(
    later.includes('Grace') && later.includes('Alan'),
    `resolved comments in the resolution chunk:\n${later.slice(0, 400)}`,
  )
  assert.ok(
    !later.includes('OX-500'),
    `no error boundary — the value reached the render callback:\n${later.slice(0, 400)}`,
  )
  assert.ok(
    later.includes('resolved-meta'),
    `resolved meta in the resolution chunk:\n${later.slice(0, 400)}`,
  )
})

test('IR defer: an unresolved placeholder streams the error boundary', async () => {
  // Same shape the Atlas `streamingError` route exercises: a rejected
  // settle maps to the core error boundary (§10.2 step 5), never a crash.
  const chunks = await drain(
    adapter.render(input({ data: { comments: { __deferred: 9 }, meta: { __deferred: 10 } } })),
  )
  const later = chunks.slice(2).join('')
  assert.ok(later.includes('data-ox-c'), `resolution chunk present:\n${later.slice(0, 300)}`)
})
