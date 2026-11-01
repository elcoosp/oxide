// Tests for the HMR runtime's `reloadModule` — the function that makes
// React Refresh in-place updates (and CSS self-accept) actually fire.
//
// ## Contract this test pins
//
// `reloadModule(url)` clears the module's accept callbacks, re-imports the
// URL with a cache-bust query, and then fires the accept callbacks the
// fresh evaluation registered. That design has one non-obvious consequence:
// **a module must re-register its accept callback on every evaluation.**
//
// In production that is guaranteed by the dev server, which prepends a
// preamble (`import.meta.hot = createHotContext("/src/foo.tsx")`) and (for
// JSX) appends an `import.meta.hot.accept(...)`. The fresh import runs the
// preamble again, re-registering at the same key. Our fixtures do the same
// thing manually, because a bare `.mjs` fixture would otherwise register
// nothing on the second evaluation and the accept would never fire.
//
// The `createHotContext` key is the **base URL** (`import.meta.url` with
// any query string stripped), which is exactly what the real preamble
// passes (a fixed literal, not `import.meta.url`, because a fresh import's
// URL carries `?t=<ms>` and would not match the original).

import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, realpathSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

type Installed = {
  createHotContext(url: string): {
    url: string
    data: Record<string, unknown>
    accept(cb?: (mod: unknown) => void): void
    dispose(cb?: () => void): void
    invalidate(): void
  }
  reloadModule(url: string): Promise<void>
}
const globals = globalThis as unknown as {
  window?: object
  __oxide_hmr__?: Installed
  __oxide_refresh?: (mod: unknown) => void
  __last_accept?: unknown
}
if (!globals.window) globals.window = globalThis

// Imported AFTER window is set so `install()` runs.
await import('../src/client/hot.ts')

function hmr(): Installed {
  const h = globals.__oxide_hmr__
  assert.ok(h, '__oxide_hmr__ should be installed by importing hot.ts')
  return h
}

// Canonicalize through the filesystem so `pathToFileURL` returns the SAME
// URL that Node's ESM loader reports as `import.meta.url`. On macOS,
// `tmpdir()` returns `/var/folders/...` (a symlink target), but Node's
// loader canonicalizes to `/private/var/folders/...`. `realpathSync`
// closes that gap. On Linux it is a no-op; on macOS it is what makes the
// fixture's self-registration key match `reloadModule`'s lookup key.
//
// In production this symlink concern does not arise: served modules receive
// a fixed-literal `createHotContext("/src/foo.tsx")` in the preamble — not
// `import.meta.url` — so a symlinked `src/` would not desync the two sides.
const dir = realpathSync(mkdtempSync(join(tmpdir(), 'oxide-hot-')))

/** A fixture that registers an accept callback at its base URL, exactly the
 *  way the dev server's preamble + `inject_hmr_boundary` arrangement does. */
function writeSelfRegisteringFixture(
  name: string,
  body = 'export const v = 1;',
): string {
  const path = join(dir, name)
  writeFileSync(
    path,
    `const __key = import.meta.url.split('?')[0]
if (typeof globalThis.__oxide_hmr__ === 'object') {
  globalThis.__oxide_hmr__.createHotContext(__key).accept((mod) => {
    globalThis.__last_accept = mod
  })
}
${body}
`,
  )
  return pathToFileURL(path).href
}

test('reloadModule re-imports and fires the fresh accept callback', async () => {
  globals.__last_accept = undefined
  const h = hmr()
  const url = writeSelfRegisteringFixture('counter.mjs')
  await h.reloadModule(url)
  assert.ok(
    globals.__last_accept !== undefined,
    'fresh accept must fire with the newly-evaluated module',
  )
  assert.equal((globals.__last_accept as { v: number }).v, 1)
})

test('reloadModule fires dispose callbacks registered before the reload', async () => {
  const h = hmr()
  const url = writeSelfRegisteringFixture('with-dispose.mjs')
  // Pre-import so the fixture registers itself and we have a module entry.
  await import(url)
  // Register a dispose on the same key. It must fire before the re-import.
  let disposed = false
  h.createHotContext(url).dispose(() => {
    disposed = true
  })
  await h.reloadModule(url)
  assert.ok(disposed, 'dispose callback must fire before the re-import')
})

test('reloadModule on a never-imported URL still works', async () => {
  const h = hmr()
  globals.__last_accept = undefined
  const url = writeSelfRegisteringFixture('fresh.mjs')
  await h.reloadModule(url)
  assert.ok(
    globals.__last_accept !== undefined,
    'a first-time reload must also fire the accept callback',
  )
})

test('two reloads fire accept exactly twice (no stacking)', async () => {
  const h = hmr()
  // A fixture that counts its own fires so stacking would be visible.
  const path = join(dir, 'stack.mjs')
  writeFileSync(
    path,
    `const __key = import.meta.url.split('?')[0]
if (typeof globalThis.__oxide_hmr__ === 'object') {
  globalThis.__oxide_hmr__.createHotContext(__key).accept(() => {
    globalThis.__reload_test_fires = (globalThis.__reload_test_fires ?? 0) + 1
  })
}
export const v = 1
`,
  )
  const url = pathToFileURL(path).href
  ;(globalThis as { __reload_test_fires?: number }).__reload_test_fires = 0
  await h.reloadModule(url)
  await h.reloadModule(url)
  const fires =
    (globalThis as { __reload_test_fires?: number }).__reload_test_fires ?? 0
  assert.equal(fires, 2, `each reload must fire exactly once, got ${fires}`)
})

test('data payload survives reloads for the same key', async () => {
  const h = hmr()
  const url = writeSelfRegisteringFixture('data.mjs')
  h.createHotContext(url).data.setup = 'one-time'
  await h.reloadModule(url)
  // Same `data` object; a setup-once pattern survives a save.
  const after = h.createHotContext(url)
  assert.equal(after.data.setup, 'one-time')
})
