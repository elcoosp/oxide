// Form enhance helper tests (spec §9.3): intercept submit, POST fetch,
// apply form result, optimistic hook.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { enhance, type EnhanceForm } from '../src/client/enhance.ts'

function fakeForm(attrs: Record<string, string> = {}) {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  const form = {
    attrs: { method: 'post', ...attrs } as Record<string, string>,
    listeners,
    dispatchLog: [] as Array<unknown>,
    addEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, [...(listeners.get(type) ?? []), fn])
    },
    removeEventListener(type: string, fn: (e: unknown) => void) {
      listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
    },
    getAttribute(n: string) {
      return n in this.attrs ? this.attrs[n] : null
    },
    hasAttribute(n: string) {
      return n in this.attrs
    },
    dispatchEvent(ev: unknown) {
      this.dispatchLog.push(ev)
      return true
    },
    dispatch(type: string, event: unknown) {
      for (const fn of listeners.get(type) ?? []) fn(event)
    },
  }
  const enriched: EnhanceForm & {
    listeners: Map<string, Array<(e: unknown) => void>>
    dispatchLog: unknown[]
    dispatch(type: string, event: unknown): void
  } = form as EnhanceForm & {
    listeners: Map<string, Array<(e: unknown) => void>>
    dispatchLog: unknown[]
    dispatch(type: string, event: unknown): void
  }
  return enriched
}

test('enhance: intercepts submit, POSTs to the action, applies the JSON result', async () => {
  const form = fakeForm({ action: '/?/comment' })
  const calls: Array<{ url: string; init: RequestInit | undefined }> = []
  const fetchImpl = (url: string | URL, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    return Promise.resolve(new Response(JSON.stringify({ ok: true, id: 9 }), { headers: { 'content-type': 'application/json' } }))
  }
  const applied: unknown[] = []
  const optimistic: FormData[] = []
  const order: string[] = []
  enhance(form, {
    fetch: fetchImpl as typeof fetch,
    getFormData: () => {
      const fd = new FormData()
      fd.set('body', 'nice post')
      return fd
    },
    onSubmit: (fd) => {
      order.push('optimistic')
      optimistic.push(fd)
    },
    applyResult: (result) => {
      order.push('apply')
      applied.push(result)
    },
  })

  let prevented = false
  form.dispatch('submit', { preventDefault: () => (prevented = true) })
  await new Promise((r) => setTimeout(r, 0))

  assert.ok(prevented, 'default submit prevented')
  assert.deepEqual(order, ['optimistic', 'apply'], 'optimistic hook runs before the fetch')
  const opt0 = optimistic[0]
  if (opt0 === undefined) throw new Error('test invariant: optimistic[0] should be defined')
  assert.equal(opt0.get('body'), 'nice post')
  assert.equal(calls.length, 1)
  const call0 = calls[0]
  if (call0 === undefined) throw new Error('test invariant: calls[0] should be defined')
  assert.equal(call0.url, '/?/comment')
  assert.equal((call0.init?.method as string).toUpperCase(), 'POST')
  assert.ok(call0.init?.body, 'request carries a body')
  assert.deepEqual(applied[0], { ok: true, id: 9 })
})

test('enhance: sends URLSearchParams for urlencoded forms, ODF results are decoded', async () => {
  const form = fakeForm({ action: '/?/like', enctype: 'application/x-www-form-urlencoded' })
  const calls: Array<RequestInit | undefined> = []
  const odf = Buffer.from(new Uint8Array([0x4f, 0x44, 0x46, 0x31, 0x00, 0x03, 0, 0, 0, 0, 0, 0, 0x10, 0x40])) // ODF1 + number 4
  const fetchImpl = () => {
    const c: RequestInit | undefined = undefined
    void c
    return Promise.resolve(new Response(odf, { headers: { 'content-type': 'application/oxide+odf' } }))
  }
  const applied: unknown[] = []
  enhance(form, {
    fetch: ((url: string | URL, init?: RequestInit) => {
      calls.push(init)
      return fetchImpl()
    }) as typeof fetch,
    getFormData: () => {
      const fd = new FormData()
      fd.set('id', '1')
      return fd
    },
    applyResult: (r) => applied.push(r),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  const body = calls[0]?.body as URLSearchParams
  assert.ok(body instanceof URLSearchParams, 'urlencoded enctype → URLSearchParams body')
  assert.equal(body.get('id'), '1')
  assert.deepEqual(applied[0], 4, 'ODF result decoded to rich values')
})

test('enhance: onError fires on failed responses; disposer detaches the listener', async () => {
  const form = fakeForm({ action: '/?/boom' })
  const errors: unknown[] = []
  let listenerCount = () => form.listeners.get('submit')?.length ?? 0
  const dispose = enhance(form, {
    fetch: (() => Promise.resolve(new Response('bad', { status: 500 }))) as typeof fetch,
    getFormData: () => new FormData(),
    onError: (e) => errors.push(e),
  })
  assert.equal(listenerCount(), 1)
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(errors.length, 1, 'error hook got the failure')

  dispose()
  assert.equal(listenerCount(), 0, 'disposer removed the submit listener')
})

test('enhance: default action is the current location', async () => {
  const form = fakeForm() // no action attribute
  const calls: string[] = []
  enhance(form, {
    fetch: ((url: string | URL) => {
      calls.push(String(url))
      return Promise.resolve(new Response('ok', { headers: { 'content-type': 'text/plain' } }))
    }) as typeof fetch,
    getFormData: () => new FormData(),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal(calls[0], '/', 'no action → POST to /')
})

test('enhance: no-JS parity — form method attribute is always overridden to POST', async () => {
  const form = fakeForm({ action: '/x', method: 'get' })
  const calls: Array<RequestInit | undefined> = []
  enhance(form, {
    fetch: ((_url: string | URL, init?: RequestInit) => {
      calls.push(init)
      return Promise.resolve(new Response('{}', { headers: { 'content-type': 'application/json' } }))
    }) as typeof fetch,
    getFormData: () => new FormData(),
  })
  form.dispatch('submit', { preventDefault() {} })
  await new Promise((r) => setTimeout(r, 0))
  assert.equal((calls[0]?.method as string).toUpperCase(), 'POST', 'actions are POST-only (§17.1)')
})
