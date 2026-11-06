// svelteRenderLoading + svelteHydrateIsland (§8.4 / §10.3) — the client contract.
//
// Svelte 5 exposes `hydrate()` as its own entry point (HANDOFF item 6:
// `mount(..., {hydrate:true})` silently appends a second tree) — the adapter
// prefers `hydrate()` when present. Runtime selection itself needs a live DOM,
// so here we pin the module contract and the live-DOM guard; real hydration is
// covered by the Playwright suite.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createFakeDom } from './helpers/fake-dom.ts'
import { svelteRenderLoading } from '../src/render-loading.ts'
import { svelteHydrateIsland } from '../src/hydrate-island-real.ts'

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

test('render-loading(svelte): returns a synchronous disposer that cancels a pending mount', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = svelteRenderLoading(overlay as unknown as Element, {}, function component() {})
    assert.equal(typeof dispose, 'function')
    dispose()
    await settle()
    dispose()
    await settle()
  })
  assert.equal(errors.length, 0, 'a cancelled mount must not surface console errors')
  assert.match(dom.html, /^<body><div id="overlay"><\/div><\/body>$/)
})

test('render-loading(svelte): an unmountable component degrades to a console error', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = svelteRenderLoading(overlay as unknown as Element, {}, null)
    await settle()
    dispose()
    await settle()
  })
  assert.ok(errors.length >= 1, 'the failure must be logged, not thrown')
  assert.match(String(errors[0]?.[0]), /renderLoading \(svelte\)/)
})

test('hydrate-island(svelte): a null module rejects with the contract TypeError', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    svelteHydrateIsland(host, null, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'hydrateIsland(svelte): island module must export a default component')
      return true
    },
  )
})

test('hydrate-island(svelte): a module WITHOUT a default falls back to the module itself (interop shape)', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // `default ?? mod` — the CJS-interop shape passes the default check and
  // proceeds until the live-DOM guard.
  await assert.rejects(
    svelteHydrateIsland(host, { exportsOnly: true }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})

test('hydrate-island(svelte): a fake-DOM host is rejected by the live-DOM guard', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    svelteHydrateIsland(host, { default: function component() {} }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})
