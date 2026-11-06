// solidRenderLoading + solidHydrateIsland (§8.4 / §10.3) — the client contract.
//
// Solid 2 keeps the web runtime in `@solidjs/web` (Solid 1: `solid-js/web`);
// the loader falls back between the two specifiers (HANDOFF item 4 — the
// original bug re-imported the same failing specifier). The web runtime IS
// resolvable in this workspace, so the live-DOM guard is the deterministic
// failure point; real hydration is covered by the Playwright suite.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createFakeDom } from './helpers/fake-dom.ts'
import { solidRenderLoading } from '../src/render-loading.ts'
import { solidHydrateIsland } from '../src/hydrate-island-real.ts'

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

test('render-loading(solid): returns a synchronous disposer that cancels a pending render', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = solidRenderLoading(overlay as unknown as HTMLElement, {}, function component() {})
    assert.equal(typeof dispose, 'function')
    dispose()
    await settle()
    dispose()
    await settle()
  })
  assert.equal(errors.length, 0, 'a cancelled render must not surface console errors')
  assert.match(dom.html, /^<body><div id="overlay"><\/div><\/body>$/)
})

test('render-loading(solid): a failed render degrades to a console error (no unhandled rejection)', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = solidRenderLoading(overlay as unknown as HTMLElement, {}, null)
    await settle()
    dispose()
    await settle()
  })
  assert.ok(errors.length >= 1, 'the failure must be logged, not thrown')
  assert.match(String(errors[0]?.[0]), /renderLoading \(solid\)/)
})

test('hydrate-island(solid): a null module rejects with the contract TypeError', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    solidHydrateIsland(host, null, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'hydrateIsland(solid): island module must export a default component')
      return true
    },
  )
})

test('hydrate-island(solid): a module WITHOUT a default falls back to the module itself (interop shape)', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // `default ?? mod` — the CJS-interop shape passes the default check and
  // proceeds until the live-DOM guard.
  await assert.rejects(
    solidHydrateIsland(host, { signalsOnly: true }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})

test('hydrate-island(solid): a fake-DOM host is rejected by the live-DOM guard', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    solidHydrateIsland(host, { default: function component() {} }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'oxide: island hydration requires a live DOM Element')
      return true
    },
  )
})
