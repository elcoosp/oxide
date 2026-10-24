// §7.1 / §9.2: the server injects a `<script type="application/oxide-deferred">`
// with resolved `defer()` values. The client glue reads it and resolves
// matching `DeferredClient` slots — the last mile of the deferred data
// protocol (the `ox:swap` event still fires for rendered boundaries, but the
// *data* path needs a script the boot module can read synchronously).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { attachDeferGlue } from '../src/client/defer-glue.ts'

function fakeDoc(scripts: Array<{ textContent: string | null }> = []): any {
  const listeners = new Map<string, Array<(e: unknown) => void>>()
  return {
    readyState: 'complete',
    addEventListener(t: string, f: (e: unknown) => void) {
      const l = listeners.get(t) ?? []
      l.push(f)
      listeners.set(t, l)
    },
    removeEventListener(t: string, f: (e: unknown) => void) {
      listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f))
    },
    querySelectorAll(sel: string) {
      return sel === 'script[type="application/oxide-deferred"]' ? scripts : []
    },
    _fire(t: string, e: unknown) {
      for (const f of listeners.get(t) ?? []) f(e)
    },
  }
}

test('defer-glue: resolved defer() values from a script resolve the slots', async () => {
  const doc = fakeDoc([{ textContent: '{"1":"hello","2":{"x":42}}' }])
  const glue = attachDeferGlue({ doc: doc })
  const data: any = { a: { __deferred: 1 }, b: { __deferred: 2 } }
  glue.wire(data)

  assert.equal(await (data.a as Promise<unknown>), 'hello', 'slot 1 resolved from script')
  assert.deepEqual(await (data.b as Promise<unknown>), { x: 42 }, 'slot 2 resolved from script')
  glue.dispose()
})

test('defer-glue: a deferred error payload rejects the slot', async () => {
  const doc = fakeDoc([
    { textContent: '{"7":{"__oxide_deferred_error":"boom"}}' },
  ])
  const glue = attachDeferGlue({ doc: doc })
  const data: any = { v: { __deferred: 7 } }
  glue.wire(data)
  await assert.rejects(() => data.v as Promise<unknown>, /boom/)
  glue.dispose()
})

test('defer-glue: a malformed script is skipped, not fatal', async () => {
  const doc = fakeDoc([
    { textContent: '{not valid json' },
    { textContent: '{"3":"ok"}' },
  ])
  const glue = attachDeferGlue({ doc: doc })
  const data: any = { v: { __deferred: 3 } }
  glue.wire(data)
  assert.equal(await (data.v as Promise<unknown>), 'ok', 'later valid script still resolves')
  glue.dispose()
})

test('defer-glue: empty textContent is a no-op', async () => {
  const doc = fakeDoc([{ textContent: '' }, { textContent: null }])
  // Must not throw, must not resolve anything.
  const glue = attachDeferGlue({ doc: doc })
  glue.dispose()
})
