// Adapter factory tests (spec §14 config surface). Proves the callable
// default export dispatches to real SSR when componentFor is supplied, and
// returns the IR adapter otherwise. The named `adapter` export stays
// unchanged, so the §22 conformance suite keeps testing the IR path.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import defaultFactory, { adapter, createAdapter } from '../src/index.ts'
import type { RenderInput } from '../../core/src/types.ts'

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

test('factory: createAdapter() defaults to real SSR (§14.1)', async () => {
  // §14.1: the literal config shape has no `ssr` field. Pre-fix, a bare
  // `createAdapter()` fell through to the IR adapter. Post-fix it routes
  // through the real adapter.
  //
  // Proof: the real adapter (unlike the IR one) consults `input.resolve`
  // — the runtime-supplied resolver that substitutes for a config-side
  // `componentFor`. The IR adapter never calls it. So `resolve` firing is
  // observable evidence the default reached real-ssr. (Real SSR in `auto`
  // mode still falls back to IR when `resolve` returns nothing and deps
  // are absent, so the rendered bytes are expected to match — that is the
  // spec's "framework-agnostic" fallback, not a regression.)
  const a = createAdapter()
  const calls: string[] = []
  const html = await drain(
    a.render(
      input({
        resolve: (id: string) => {
          calls.push(id)
          return undefined
        },
      }),
    ),
  )
  assert.ok(calls.length > 0, `default reached real SSR (resolve consulted): ${calls.length} call(s)`)
  assert.ok(html.startsWith('<!doctype html>'), 'auto-mode fallback still renders a page')
})

test('factory: createAdapter({ ssr: "ir" }) is the explicit IR escape hatch', async () => {
  // The pre-§14.1 behavior, opt-in. Bypasses the real adapter entirely.
  const a = createAdapter({ ssr: 'ir' })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  const irHtml = await drain(adapter.render(input({ route: 'routes/missing' })))
  assert.equal(html, irHtml, 'ssr:ir == named `adapter` byte-for-byte')
})

test('factory: createAdapter({ ssr: "real", componentFor: () => undefined }) emits the OX-500 page', async () => {
  const a = createAdapter({
    ssr: 'real',
    componentFor: () => undefined,
  })
  const html = await drain(a.render(input({ route: 'routes/missing' })))
  // §6.4 / §10.2 step 5: a missing page is a diagnostic + a 500, never
  // a silent comment. The marker is what the pipeline promotes to HTTP
  // 500; the div is the visible fallback.
  assert.ok(html.includes('<!--oxide-status:500-->'), `500 marker: ${html}`)
  assert.ok(html.includes("data-ox-error='OX-500'"), `OX-500 div: ${html}`)
  assert.ok(html.includes('no component resolved for route routes/missing'), `diagnostic wording: ${html}`)
})

test('factory: componentFor + ssr="auto" with missing component falls back to IR', async () => {
  const a = createAdapter({
    ssr: 'auto',
    componentFor: () => undefined,
  })
  const html = await drain(a.render(input()))
  assert.ok(html.startsWith('<!doctype html>'), html)
})

test('factory: AdapterOptions accepts a compiler flag without breaking dispatch', () => {
  // The smoke-app config passes `{ compiler: true }`; the factory must accept
  // it (typed as informational) and still return a valid adapter.
  const a = defaultFactory({ compiler: true })
  assert.equal(a.contract, 1)
})
