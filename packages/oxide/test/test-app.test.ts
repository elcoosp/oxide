// createTestApp: in-process fetch helper (spec §16.6, §22 "app.fetch()").
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createTestApp } from '../src/test.ts'

test('test app: normalizes {method,url,headers,body} into a Request', async () => {
  const seen: Request[] = []
  const app = createTestApp((req) => {
    seen.push(req)
    return new Response(`ok:${req.method}:${new URL(req.url).pathname}:${req.headers.get('x-test')}`, { status: 201 })
  })
  const res = await app.fetch({
    method: 'post',
    url: 'http://localhost/?/comment',
    headers: { 'x-test': '1' },
    body: 'hello',
  })
  assert.equal(res.status, 201)
  assert.equal(await res.text(), 'ok:POST:/:1') // '?/comment' is the query (action name)
  const req = seen[0]
  assert.ok(req instanceof Request)
  assert.equal(req.method, 'POST')
  assert.equal(await req.text(), 'hello')
})

test('test app: body without explicit method defaults to POST; string input is GET', async () => {
  const app = createTestApp((req) => new Response(req.method))
  assert.equal(await (await app.fetch('http://localhost/x')).text(), 'GET')
  assert.equal(await (await app.fetch({ url: 'http://localhost/x', body: 'data' })).text(), 'POST')
  assert.equal(await (await app.fetch(new Request('http://localhost/x', { method: 'PUT' }))).text(), 'PUT')
})

test('test app: async handlers and JSON round-trip', async () => {
  const app = createTestApp(async (req) => {
    const data = req.method === 'POST' ? await req.json() : null
    return Response.json({ echo: data, route: new URL(req.url).searchParams.get('route') })
  })
  const res = await app.fetch({ method: 'POST', url: 'http://localhost/__data?route=/blog/a', body: JSON.stringify({ n: 1 }) })
  assert.deepEqual(await res.json(), { echo: { n: 1 }, route: '/blog/a' })
})

test('test app: handler errors propagate to the caller', async () => {
  const app = createTestApp(() => {
    throw new Error('load blew up')
  })
  await assert.rejects(() => app.fetch('http://localhost/'), /load blew up/)
})

test('test app: end-to-end with an Oxide load pipeline shape (deferred data)', async () => {
  // a tiny handler mimicking the server pipeline: load → ODF bytes via the
  // test-local lite encoder (deferred ids inline), asserting §16.6 ergonomics
  const { encodeLite } = await import('./helpers/odf-lite.ts')
  const app = createTestApp((req) => {
    const route = new URL(req.url).searchParams.get('route') ?? '/'
    return new Response(encodeLite({ route, params: {}, data: { title: 'Post', later: { __deferred: 1 } } }), {
      headers: { 'content-type': 'application/oxide+odf' },
    })
  })
  const res = await app.fetch('http://localhost/__data?route=%2Fblog%2Fa')
  assert.equal(res.headers.get('content-type'), 'application/oxide+odf')
  const { decodeOdf } = await import('../src/client/odf-decode.ts')
  const data = decodeOdf(new Uint8Array(await res.arrayBuffer())) as { route: string; data: { later: { __deferred: number } } }
  assert.equal(data.route, '/blog/a')
  assert.deepEqual(data.data.later, { __deferred: 1 })
})
