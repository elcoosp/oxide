// §9.4 server functions — the client-side `__oxide_call_fn` contract.
//
// The compiler rewrites `serverFn(...)` in `*.server.ts` modules to stubs
// that call this module, so the fetch contract here IS the wire contract:
//
//   POST /_oxide/fn/<url-encoded-id>
//   content-type: application/oxide-fn
//   body: JSON.stringify(args)   — the BARE argument array
//
// The body shape matters: the worker op (`server-fn:run`) JSON.parses the
// body directly as the argument list — wrapping it in `{args: …}` makes the
// worker reject a valid call with "arguments must be a JSON array" (see the
// module comment in src/fn.ts). These tests pin all of it with a stubbed
// global fetch (no network, no runtime deps).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { __oxide_call_fn } from '../src/fn.ts'

/** Stub the global fetch and record every call it sees. Returns the calls
 *  plus a restore function — call restore in a `finally` so one test's
 *  monkeypatching never leaks into the next. */
interface RecordedCall {
  url: string
  init: RequestInit | undefined
}

interface FetchStub {
  calls: RecordedCall[]
  restore(): void
}

function stubFetch(handler: (url: string, init: RequestInit | undefined) => Response): FetchStub {
  const calls: RecordedCall[] = []
  const original = globalThis.fetch
  globalThis.fetch = ((url: string | URL | Request, init?: RequestInit) => {
    const u = typeof url === 'string' ? url : url instanceof Request ? url.url : url.toString()
    calls.push({ url: u, init })
    return handler(u, init)
  }) as unknown as typeof fetch
  return {
    calls,
    restore() {
      globalThis.fetch = original
    },
  }
}

function jsonResponse(body: string, status = 200): Response {
  return new Response(body, {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

test('server fn: POSTs to /_oxide/fn/<id> with the oxide-fn content type', async () => {
  const stub = stubFetch(() => jsonResponse(JSON.stringify({ ok: true, data: 42 })))
  try {
    const out = await __oxide_call_fn('add', [1, 2])
    assert.equal(out, 42)
    assert.equal(stub.calls.length, 1)
    const call = stub.calls[0]
    if (!call) throw new Error('expected one recorded fetch call')
    assert.equal(call.url, '/_oxide/fn/add')
    assert.equal(call.init?.method, 'POST')
    const headers = new Headers(call.init?.headers)
    assert.equal(headers.get('content-type'), 'application/oxide-fn')
  } finally {
    stub.restore()
  }
})

test('server fn: body is the BARE argument array (worker contract, §9.4)', async () => {
  const stub = stubFetch(() => jsonResponse(JSON.stringify({ ok: true, data: null })))
  try {
    await __oxide_call_fn('add', [1, 'two', { nested: true }])
    const call = stub.calls[0]
    if (!call) throw new Error('expected one recorded fetch call')
    assert.equal(call.init?.body, JSON.stringify([1, 'two', { nested: true }]))
    assert.deepEqual(JSON.parse(String(call.init?.body)), [1, 'two', { nested: true }])
  } finally {
    stub.restore()
  }
})

test('server fn: empty args still serialize as an array, not an object', async () => {
  const stub = stubFetch(() => jsonResponse(JSON.stringify({ ok: true, data: 'x' })))
  try {
    await __oxide_call_fn('zero', [])
    const call = stub.calls[0]
    if (!call) throw new Error('expected one recorded fetch call')
    assert.equal(call.init?.body, '[]')
  } finally {
    stub.restore()
  }
})

test('server fn: the id is URL-encoded in the path', async () => {
  const stub = stubFetch(() => jsonResponse(JSON.stringify({ ok: true, data: null })))
  try {
    await __oxide_call_fn('weird id/with slash', [])
    const call = stub.calls[0]
    if (!call) throw new Error('expected one recorded fetch call')
    assert.equal(call.url, `/_oxide/fn/${encodeURIComponent('weird id/with slash')}`)
    assert.ok(!call.url.includes(' '), 'raw space must not reach the URL')
  } finally {
    stub.restore()
  }
})

test('server fn: ok:false rejects with the server message and carries the code', async () => {
  const stub = stubFetch(() =>
    jsonResponse(JSON.stringify({ ok: false, code: 'FN-NOT-FOUND', message: 'no such fn' })))
  try {
    await assert.rejects(
      __oxide_call_fn('ghost', []),
      (e: unknown) => {
        assert.ok(e instanceof Error)
        assert.equal(e.message, 'no such fn')
        assert.equal((e as Error & { code?: string }).code, 'FN-NOT-FOUND')
        return true
      },
    )
  } finally {
    stub.restore()
  }
})

test('server fn: a non-JSON body surfaces the status and a body prefix', async () => {
  const stub = stubFetch(() => new Response('<html>gateway timeout</html>', { status: 504 }))
  try {
    await assert.rejects(
      __oxide_call_fn('add', []),
      (e: unknown) => {
        assert.ok(e instanceof Error)
        assert.ok(e.message.includes('504'), 'message includes the HTTP status')
        assert.ok(e.message.includes('non-JSON body'), 'message names the non-JSON body case')
        assert.ok(e.message.includes('gateway timeout'), 'message includes a body prefix')
        return true
      },
    )
  } finally {
    stub.restore()
  }
})

test('server fn: error bodies are decoded even on non-2xx statuses', async () => {
  // The module comment promises "any non-2xx carries the same body shape so
  // the caller gets a useful message instead of a bare fetch failure".
  const stub = stubFetch(() =>
    jsonResponse(JSON.stringify({ ok: false, code: 'FN-PANIC', message: 'boom on the server' }), 500))
  try {
    await assert.rejects(
      __oxide_call_fn('add', []),
      (e: unknown) => {
        assert.ok(e instanceof Error)
        assert.equal(e.message, 'boom on the server')
        assert.equal((e as Error & { code?: string }).code, 'FN-PANIC')
        return true
      },
    )
  } finally {
    stub.restore()
  }
})

test('server fn: data passes through unchanged (objects, arrays, null)', async () => {
  const payload = { list: [1, 2, 3], nested: { deep: true }, none: null }
  const stub = stubFetch(() => jsonResponse(JSON.stringify({ ok: true, data: payload })))
  try {
    const out = (await __oxide_call_fn('fetch-post', [7])) as typeof payload
    assert.deepEqual(out, payload)
  } finally {
    stub.restore()
  }
})
