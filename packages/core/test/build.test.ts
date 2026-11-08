// §V2-A04 step 4 — the client build-id contract: withBuild stamps the
// advertised `window.__oxide.build` onto data-plane fetches, handleStale
// turns a stale-generation 409 into one guarded reload.
//
// Written FIRST (TDD red): these pins failed against the missing
// `packages/core/src/client/build.ts` module, then the module and the
// router/enhance/fn wiring brought them green.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildId, handleStale, withBuild } from '../src/client/build.ts'
import { enhance, type EnhanceForm } from '../src/client/enhance.ts'
import { __oxide_call_fn } from '../src/fn.ts'
import { encodeLite } from './helpers/odf-lite.ts'

/** Set/clear the advertised bootstrap; returns a disposer. */
function advertise(build: string | undefined): () => void {
  const g = globalThis as { __oxide?: { route?: string; build?: string } }
  const prev = g.__oxide
  g.__oxide = build === undefined ? undefined : { route: 'routes/index', build }
  return () => {
    g.__oxide = prev
  }
}

/** Install a fake sessionStorage; returns the store and a disposer. */
function fakeSessionStorage(): { store: Map<string, string>; restore: () => void } {
  const store = new Map<string, string>()
  const g = globalThis as { sessionStorage?: unknown }
  const prev = g.sessionStorage
  g.sessionStorage = {
    getItem: (k: string) => (store.has(k) ? (store.get(k) as string) : null),
    setItem: (k: string, v: string) => void store.set(k, v),
  }
  return { store, restore: () => void (g.sessionStorage = prev) }
}

function staleResponse(): Response {
  return new Response(null, { status: 409, headers: { 'x-oxide-build-stale': 'true' } })
}

test('buildId reads the advertised window.__oxide.build', () => {
  const restore = advertise('deadbeef0000000f')
  try {
    assert.equal(buildId(), 'deadbeef0000000f')
  } finally {
    restore()
  }
  assert.equal(buildId(), undefined, 'undefined without an advertised id')
})

test('withBuild adds the header only when __oxide.build exists', () => {
  assert.equal(withBuild({}).headers, undefined, 'no advertised id: init untouched')

  const restore = advertise('deadbeef0000000f')
  try {
    const init = withBuild({})
    assert.equal(new Headers(init.headers).get('x-oxide-build'), 'deadbeef0000000f')
  } finally {
    restore()
  }
})

test('withBuild merges into existing headers without dropping them', () => {
  const restore = advertise('aa')
  try {
    const init = withBuild({ headers: { accept: 'application/oxide+odf' } })
    const h = new Headers(init.headers)
    assert.equal(h.get('accept'), 'application/oxide+odf')
    assert.equal(h.get('x-oxide-build'), 'aa')
  } finally {
    restore()
  }
})

test('withBuild accepts Headers instances too', () => {
  const restore = advertise('bb')
  try {
    const incoming = new Headers({ accept: 'application/json' })
    const h = new Headers(withBuild({ headers: incoming }).headers)
    assert.equal(h.get('x-oxide-build'), 'bb')
    assert.equal(h.get('accept'), 'application/json')
  } finally {
    restore()
  }
})

test('handleStale ignores non-409 and non-stale responses', () => {
  let reloads = 0
  const reload = (): void => void reloads++
  assert.equal(handleStale(new Response(null, { status: 200 }), reload), false)
  // 409 without the stale marker (an unrelated 409) must NOT reload.
  assert.equal(handleStale(new Response(null, { status: 409 }), reload), false)
  assert.equal(reloads, 0)
})

test('handleStale reloads once per build id and never loops', () => {
  const { store, restore } = fakeSessionStorage()
  const restoreBuild = advertise('deadbeef0000000f')
  let reloads = 0
  try {
    assert.equal(handleStale(staleResponse(), () => void reloads++), true)
    assert.equal(reloads, 1)
    assert.equal(store.size, 1, 'the guard key was written')
    // The second 409 for the SAME build id does not reload again.
    assert.equal(handleStale(staleResponse(), () => void reloads++), false)
    assert.equal(reloads, 1)
  } finally {
    restore()
    restoreBuild()
  }
})

test('handleStale reloads again when a NEW build id goes stale', () => {
  const { restore } = fakeSessionStorage()
  let reloads = 0
  try {
    const a = advertise('aaa')
    handleStale(staleResponse(), () => void reloads++)
    a()
    const b = advertise('bbb')
    handleStale(staleResponse(), () => void reloads++)
    b()
    assert.equal(reloads, 2, 'a new generation gets its own one-shot guard')
  } finally {
    restore()
  }
})

test('handleStale without storage reloads once (degraded, no loop guard)', () => {
  const g = globalThis as { sessionStorage?: unknown }
  const prev = g.sessionStorage
  delete g.sessionStorage // forces the "no storage" path in Node
  let reloads = 0
  try {
    assert.equal(handleStale(staleResponse(), () => void reloads++), true)
    assert.equal(reloads, 1)
  } finally {
    g.sessionStorage = prev
  }
})

test('the /__data router fetch carries x-oxide-build and aborts stale navigations', async () => {
  // Minimal §8.4 DOM fakes (same shapes as router.test.ts, trimmed).
  const doc = {
    addEventListener() {},
    removeEventListener() {},
    querySelectorAll: () => [] as never[],
  }
  const history = {
    state: { __oxide: 1 as const, input: { route: '/', data: null }, scroll: { x: 0, y: 0 } },
    pushState() {},
    replaceState() {},
  }
  const location = { href: 'http://x.test/', pathname: '/', search: '' }
  const adapter = { update() {} }

  const restoreBuild = advertise('routerbuild1')
  let seenInit: RequestInit | undefined
  let seenUrl = ''
  try {
    const { createRouter } = await import('../src/client/router.ts')
    const okRouter = createRouter({
      adapter,
      doc: doc as never,
      history: history as never,
      location: location as never,
      fetch: (async (input: unknown, init?: RequestInit) => {
        seenUrl = String(input)
        seenInit = init
        return new Response(encodeLite({ route: '/', params: {}, url: 'http://x.test/', data: {} }))
      }) as typeof fetch,
    })
    await okRouter.goto('/')
    assert.equal(seenUrl, '/__data?route=%2F')
    assert.equal(new Headers(seenInit?.headers).get('x-oxide-build'), 'routerbuild1')

    // Stale generation: the fetch result must never reach decode — the
    // navigation rejects and the one-shot guard was written.
    const { store, restore } = fakeSessionStorage()
    const staleRouter = createRouter({
      adapter,
      doc: doc as never,
      history: history as never,
      location: location as never,
      fetch: (async () => staleResponse()) as typeof fetch,
    })
    try {
      await assert.rejects(() => staleRouter.goto('/'), /stale build/)
      assert.equal(store.size, 1, 'the one-shot guard was written')
    } finally {
      restore()
    }
  } finally {
    restoreBuild()
  }
})

test('the action fetch (enhance) carries x-oxide-build and skips parse on stale', async () => {
  const listeners = new Map<string, (e: unknown) => void>()
  const form = {
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, fn)
    },
    removeEventListener() {},
    getAttribute(n: string) {
      return n === 'enctype' ? null : null
    },
    hasAttribute() {
      return false
    },
  } as EnhanceForm
  const submit = (): Promise<void> =>
    (listeners.get('submit') as (e: unknown) => Promise<void>)({ preventDefault() {} })

  const restoreBuild = advertise('enhancebuild1')
  let seenInit: RequestInit | undefined
  try {
    const doFetch = (async (_input: unknown, init?: RequestInit) => {
      seenInit = init
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }) as typeof fetch
    enhance(form, { fetch: doFetch, getFormData: () => new FormData() })
    await submit()
    assert.equal(new Headers(seenInit?.headers).get('x-oxide-build'), 'enhancebuild1')

    // Stale: the submit resolves silently (a reload is in flight), no
    // error event fires, and the response body is never parsed.
    const { store, restore } = fakeSessionStorage()
    const staleFetch = (async () => staleResponse()) as typeof fetch
    const errors: unknown[] = []
    enhance(form, {
      fetch: staleFetch,
      getFormData: () => new FormData(),
      onError: (e) => void errors.push(e),
    })
    await submit()
    restore()
    assert.equal(errors.length, 0, 'a stale rejection is a reload, not a form error')
    assert.equal(store.size, 1)
  } finally {
    restoreBuild()
  }
})

test('the server-fn call carries x-oxide-build and aborts stale calls', async () => {
  const restoreBuild = advertise('fnbuild1')
  const prevFetch = globalThis.fetch
  let seenInit: RequestInit | undefined
  try {
    globalThis.fetch = (async (_input: unknown, init?: RequestInit) => {
      seenInit = init
      return new Response(JSON.stringify({ ok: true, data: 7 }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }) as typeof fetch
    assert.equal(await __oxide_call_fn('add', [1, 2]), 7)
    assert.equal(new Headers(seenInit?.headers).get('x-oxide-build'), 'fnbuild1')

    const { store, restore } = fakeSessionStorage()
    globalThis.fetch = (async () => staleResponse()) as typeof fetch
    await assert.rejects(() => __oxide_call_fn('add', [1, 2]), /stale build/)
    restore()
    assert.equal(store.size, 1, 'the fn path writes the one-shot guard too')
  } finally {
    globalThis.fetch = prevFetch
    restoreBuild()
  }
})
