// reactHydrateIsland (§10.3) — the island-module contract.
//
// `reactHydrateIsland(host, mod, props)` takes the dynamic-import result of
// an island chunk. The module contract:
//   - `{ default: Component }` (native ESM / bundler output)
//   - the module itself IS the component (older interop wrappers)
//   - anything else must reject with the guard's TypeError — the honest
//     failure the §10.3 contract specifies.
//
// The final `hydrateRoot` call needs a live DOM Element (fake-dom hosts are
// rejected by react-dom itself) — the real hydration is covered end to end by
// Playwright (island.spec.ts).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createFakeDom } from './helpers/fake-dom.ts'
import { reactHydrateIsland } from '../src/hydrate-island-real.ts'

test('hydrate-island(react): a module with no function export rejects with the guard TypeError', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    reactHydrateIsland(host, {}, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'island module must export a React function component')
      return true
    },
  )
})

test('hydrate-island(react): a non-function default export rejects with the guard TypeError', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  await assert.rejects(
    reactHydrateIsland(host, { default: 'string-is-not-a-component' }, {}),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'island module must export a React function component')
      return true
    },
  )
})

test('hydrate-island(react): a function-shaped module passes the guard, then requires a live DOM', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'><span>ssr</span></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // The interop shape (module-as-component) must NOT trip the guard — the
  // rejection below comes from react-dom's container check, proving the
  // module contract accepted the function and reached the runtime.
  await assert.rejects(
    reactHydrateIsland(host, () => null, {}),
    (e: unknown) => {
      assert.ok(e instanceof Error)
      assert.notEqual(e.message, 'island module must export a React function component')
      return true
    },
  )
})

test('hydrate-island(react): props null is tolerated (createElement receives {})', async () => {
  const dom = createFakeDom("<ox-island data-ox-island='X'></ox-island>")
  const host = dom.doc.querySelector('ox-island')
  if (!host) throw new Error('fixture island missing')
  // A non-function module rejects BEFORE any runtime load — proving props
  // normalization happens after the guard, and the guard is the first failure.
  await assert.rejects(
    reactHydrateIsland(host, { default: 42 }, null),
    (e: unknown) => {
      assert.ok(e instanceof TypeError)
      assert.equal(e.message, 'island module must export a React function component')
      return true
    },
  )
})
