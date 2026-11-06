// vueRenderLoading + vueHydrateIsland (§8.4 / §10.3) — the client contract.
//
// The disposer/cancel/logging contract mirrors the React adapter; Vue's own
// `createSSRApp().mount()` hydration needs a live DOM Element, which the
// adapter guards explicitly (`asDomElement`) — the guard TypeError is part of
// the contract the islands runtime relies on. Real hydration is covered by
// the Playwright suite.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createFakeDom } from './helpers/fake-dom.ts'
import { vueRenderLoading } from '../src/render-loading.ts'
import { vueHydrateIsland } from '../src/hydrate-island-real.ts'

async function captureConsoleError(fn: () => Promise<void> | void): Promise<unknown[][]> {
  const calls: unknown[][] = []
  const original = console.error
  console.error = (...args: unknown[]) => {
    calls.push(args)
  }
  try {
    await fn()
  } finally {
    console.error = original
  }
  return calls
}

async function settle(): Promise<void> {
  await new Promise<void>((r) => setTimeout(r, 30))
}

test('render-loading(vue): returns a synchronous disposer that cancels a pending mount', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = vueRenderLoading(overlay as unknown as Element, {}, { template: '<p>loading</p>' })
    assert.equal(typeof dispose, 'function')
    dispose()
    await settle()
    dispose()
    await settle()
  })
  assert.equal(errors.length, 0, 'a cancelled mount must not surface console errors')
  assert.match(dom.html, /^<body><div id="overlay"><\/div><\/body>$/)
})

test('render-loading(vue): an unmountable component degrades to a console error', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = vueRenderLoading(overlay as unknown as Element, {}, null)
    await settle()
    dispose()
    await settle()
  })
  assert.ok(errors.length >= 1, 'the failure must be logged, not thrown')
  assert.match(String(errors[0]?.[0]), /renderLoading \(vue\)/)
})

test('hydrate-island(vue): a null module rejects with the contract TypeError', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    vueHydrateIsland(host, null, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'hydrateIsland(vue): island module must export a default component')
      return true
    },
  )
})

test('hydrate-island(vue): a module WITHOUT a default falls back to the module itself (interop shape)', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // `default ?? mod` — older CJS interop wrappers ship the component as the
  // module itself. Such a module passes the default check and proceeds until
  // the live-DOM guard; it must NOT be rejected as "no default".
  await assert.rejects(
    vueHydrateIsland(host, { noDefaultHere: true }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})

test('hydrate-island(vue): a fake-DOM host is rejected by the live-DOM guard', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // The component exists; Vue's hydration requires a real Element, and the
  // adapter says so instead of feeding the fake to the runtime.
  await assert.rejects(
    vueHydrateIsland(host, { default: { template: '<p>x</p>' } }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})
