// Server runtime API tests (spec §9.1-§9.4, §16.6, §17.1).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  defineLoad, defineAction, defineMeta, defineSocket,
  defer, error, redirect, cache, cacheKey, cacheTag, cacheLife,
  serverFn, revalidate,
  OxideHttpError, OxideRedirect,
  getServerFn, __oxide_fn_registry, _revalidateLog,
} from '../src/server/index.ts'

test('defineLoad tags the fn and exposes the LoadEvent surface', async () => {
  const load = defineLoad(async (event) => {
    assert.equal(typeof event.params, 'object')
    assert.ok(event.request instanceof Request)
    assert.equal(typeof event.locals, 'object')
    assert.equal(typeof event.cookies.get, 'function')
    assert.equal(typeof event.cookies.set, 'function')
    assert.equal(typeof event.setHeaders, 'function')
    assert.equal(typeof event.parent, 'function')
    assert.equal(typeof event.defer, 'function')
    return { hello: 'world' }
  })
  assert.equal((load as { $$oxide?: string }).$$oxide, 'load')
  const event = makeEvent()
  const data = await load(event)
  assert.deepEqual(data, { hello: 'world' })
})

test('defer(promise) yields a marker; defineLoad serializes it to the Value::Deferred shape', async () => {
  // standalone marker shape (string key, Symbol-safe)
  const marker = defer(Promise.resolve(42))
  assert.equal(marker.__oxide_defer, true)
  assert.ok(marker.promise instanceof Promise)

  // event-scoped defer assigns a streaming id immediately
  const load = defineLoad(async ({ defer: eventDefer }) => {
    return { post: {}, comments: eventDefer(Promise.resolve([1, 2])) }
  })
  const deferred = new Map()
  const event = makeEvent({ deferred })
  const data = await load(event)
  assert.deepEqual(data, { post: {}, comments: { __deferred: 1 } })
  assert.equal(deferred.size, 1)
  assert.deepEqual(await deferred.get(1), [1, 2])

  // standalone markers returned inside data are also converted on the way out
  const load2 = defineLoad(async () => ({ later: defer(Promise.resolve('x')) }))
  const ev2 = makeEvent()
  const out = await load2(ev2)
  assert.deepEqual(out.later, { __deferred: 1 })
  const registryOut = (ev2 as unknown as { __oxideDeferredRegistry?: Map<number, Promise<unknown>> }).__oxideDeferredRegistry
  assert.equal(await registryOut?.get(1), 'x')
})

test('error() throws an OxideHttpError sentinel with a status', () => {
  assert.throws(() => error(404), (e: unknown) => {
    assert.ok(e instanceof OxideHttpError)
    assert.equal(e.status, 404)
    assert.equal(e.message, 'Not Found')
    return true
  })
  assert.throws(() => error(418, "I'm a teapot"), (e: unknown) => {
    assert.ok(e instanceof OxideHttpError)
    assert.equal(e.status, 418)
    assert.equal(e.message, "I'm a teapot")
    return true
  })
})

test('redirect() throws an OxideRedirect sentinel (303 default)', () => {
  assert.throws(() => redirect('/login'), (e: unknown) => {
    assert.ok(e instanceof OxideRedirect)
    assert.equal(e.location, '/login')
    assert.equal(e.status, 303)
    return true
  })
  assert.throws(() => redirect('/moved', 308), (e: unknown) => {
    assert.ok(e instanceof OxideRedirect)
    assert.equal(e.status, 308)
    return true
  })
})

test('defineAction/defineMeta/defineSocket shapes', async () => {
  const action = defineAction({
    input: {
      '~standard': {
        validate: (v: unknown) => ({ value: v }),
      },
    },
    async run({ input }) {
      return { echoed: input }
    },
  })
  assert.equal(action.$$oxide, 'action')
  assert.equal(typeof action.run, 'function')
  const actionEvent = {
    params: {},
    request: new Request('http://x.test/'),
    locals: {},
    cookies: { get: () => undefined, set: () => {}, delete: () => {}, getAll: () => [] },
    form: new FormData(),
    revalidate: { tag: () => {}, path: () => {} },
    input: { body: 'hi' },
  }
  assert.deepEqual(await action.run(actionEvent), { echoed: { body: 'hi' } })

  const meta = defineMeta(
    ({ data }: { data: unknown; params: Record<string, string | string[]> }) => ({
      title: (data as { title: string }).title,
    }),
  )
  assert.equal((meta as { $$oxide?: string }).$$oxide, 'meta')
  assert.deepEqual(meta({ data: { title: 'T' }, params: {} }), { title: 'T' })

  const socket = defineSocket({ open() {}, message() {}, close() {} })
  assert.equal(socket.$$oxide, 'socket')
  assert.deepEqual(Object.keys(socket).filter((k) => k !== '$$oxide').sort(), ['close', 'message', 'open'])
})

test('cache(): keyed by args+opts, ttl respected, tag invalidation', async () => {
  let calls = 0
  const fn = cache(async (a: string, b: number) => {
    calls++
    return `${a}-${b}-${calls}`
  }, { ttl: 60, tags: ['posts'] })

  assert.equal(await fn('x', 1), 'x-1-1')
  assert.equal(await fn('x', 1), 'x-1-1') // cached
  assert.equal(calls, 1)
  assert.equal(await fn('x', 2), 'x-2-2') // different args → new entry
  assert.equal(calls, 2)
})

test('cacheKey(): deterministic, arg-sensitive, opts-sensitive', () => {
  const k1 = cacheKey(['a', 1], { ttl: 60, tags: ['p', 'q'] })
  const k2 = cacheKey(['a', 1], { ttl: 60, tags: ['q', 'p'] }) // tag order normalized
  const k3 = cacheKey(['a', 2], { ttl: 60, tags: ['p', 'q'] })
  const k4 = cacheKey(['a', 1], { ttl: 120, tags: ['p', 'q'] })
  assert.equal(k1, k2, 'tags are sorted before hashing')
  assert.notEqual(k1, k3)
  assert.notEqual(k1, k4)
  assert.equal(cacheKey(['a', 1]), cacheKey(['a', 1]))
})

test('cacheTag/cacheLife produce documented markers', () => {
  assert.deepEqual(cacheTag('posts:1', 'posts'), { __oxideCacheTag: ['posts:1', 'posts'] })
  assert.deepEqual(cacheLife({ ttl: 30, swr: 600 }), { __oxideCacheLife: { ttl: 30, swr: 600 } })
})

test('serverFn(): tagged, registered by given id in __oxide_fn_registry', async () => {
  const addToCart = serverFn('cart.server.ts#addToCart', async (id: string) => ({ added: id }))
  assert.equal((addToCart as { $$oxideServerFn?: boolean }).$$oxideServerFn, true)
  assert.ok(__oxide_fn_registry['cart.server.ts#addToCart'] === addToCart)
  assert.ok(getServerFn('cart.server.ts#addToCart') === addToCart)
  assert.deepEqual(await addToCart('p1'), { added: 'p1' })
  // anonymous fallback still registers (Rust computes real ids at build)
  const anon = serverFn(async () => 7)
  assert.equal((anon as { $$oxideServerFn?: boolean }).$$oxideServerFn, true)
  assert.deepEqual(await anon(), 7)
})

test('revalidate: tag/path collectors record their calls', () => {
  _revalidateLog.length = 0
  assert.deepEqual(revalidate.tag('comments:post-1'), { type: 'tag', tag: 'comments:post-1' })
  assert.deepEqual(revalidate.path('/blog/a'), { type: 'path', path: '/blog/a' })
  assert.deepEqual([..._revalidateLog], [
    { type: 'tag', tag: 'comments:post-1' },
    { type: 'path', path: '/blog/a' },
  ])
})

// ---- helpers ----

function makeEvent(extra: { deferred?: Map<number, Promise<unknown>> } = {}) {
  // the wrapper adopts the event's __oxideDeferredRegistry if present
  const registry = extra.deferred ?? new Map<number, Promise<unknown>>()
  return {
    params: {},
    request: new Request('http://localhost/'),
    locals: {},
    cookies: {
      get: (_name: string) => undefined,
      set: (_name: string, _value: string, _opts?: unknown) => {},
    },
    __oxideDeferredRegistry: registry,
    defer: (p: Promise<unknown>) => {
      const id = registry.size + 1
      registry.set(id, p)
      return { __deferred: id }
    },
    setHeaders: (_h: Record<string, string>) => {},
    parent: async () => ({}),
  }
}
