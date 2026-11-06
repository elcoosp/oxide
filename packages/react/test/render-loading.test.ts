// reactRenderLoading (§8.4) — the client-side loading-boundary contract.
//
// `reactRenderLoading(overlay, input, Component)` must:
//   1. return a synchronous disposer (the router calls it before the tree
//      swap; `onNavigateStart` is sync),
//   2. mount nothing when the disposer fires before the async runtime load
//      settles (navigation superseded — no stray overlay content, no error),
//   3. degrade to a console error (never an unhandled rejection) when the
//      component is not a function or the container is not a live DOM node.
//
// The happy path (real DOM + real React mount) is covered by the Playwright
// suite (loading.spec.ts) — this file pins the contract the router depends on.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createFakeDom } from './helpers/fake-dom.ts'
import { reactRenderLoading } from '../src/render-loading.ts'

/** Capture console.error for the duration of `fn` (restored in finally). */
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

/** Settle a task + microtask queue so the module's async mount path has run
 *  (or has been cancelled) before assertions. */
async function settle(): Promise<void> {
  await new Promise<void>((r) => setTimeout(r, 30))
}

test('render-loading(react): returns a synchronous disposer that cancels a pending mount', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')
  const Component = () => null

  const errors = await captureConsoleError(async () => {
    const dispose = reactRenderLoading(overlay, {}, Component)
    assert.equal(typeof dispose, 'function', 'disposer must be synchronous')
    dispose()
    await settle()
    dispose() // idempotent
    await settle()
  })
  assert.equal(errors.length, 0, 'a cancelled mount must not surface console errors')
  assert.match(dom.html, /^<body><div id="overlay"><\/div><\/body>$/)
})

test('render-loading(react): a non-function component degrades to a console error', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  const errors = await captureConsoleError(async () => {
    const dispose = reactRenderLoading(overlay, {}, 'not-a-component')
    await settle()
    dispose()
    await settle()
  })
  assert.ok(errors.length >= 1, 'the failure must be logged, not thrown')
  assert.match(String(errors[0]?.[0]), /renderLoading \(react\)/)
})

test('render-loading(react): disposer after a failed mount attempt stays safe', async () => {
  const dom = createFakeDom('<div id="overlay"></div>')
  const overlay = dom.doc.querySelector('#overlay')
  if (!overlay) throw new Error('fixture overlay missing')

  // The container is a fake DOM element — the real react-dom runtime rejects
  // it, the catch path logs, and the disposer must still be callable.
  const errors = await captureConsoleError(async () => {
    const dispose = reactRenderLoading(overlay, {}, () => null)
    await settle()
    dispose()
    await settle()
  })
  assert.ok(errors.length >= 1, 'incompatible container surfaces a logged error')
  assert.match(String(errors[0]?.[0]), /renderLoading \(react\)/)
})
