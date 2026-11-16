// §V2-E01: the Solid component-refresh shim + the in-place refresh
// registry — the client runtime half of the Vue/Solid fast-refresh wiring.
//
// The compile step (the worker's `solid-compile` op) emits the accept
// boundary calling `__oxide_solid_refresh(mod)`; this test pins the shim's
// contract:
//
// - With a solid-refresh runtime on the global (`$SolidRefreshRuntime$` —
//   the probe-first prebundle pull, mirroring the React Refresh runtime),
//   the shim delegates to `performSolidRefresh(mod)`: the state-preserving
//   in-place path.
// - Without a runtime, the shim does NOTHING and marks nothing: the boot
//   replay (`__oxide_on_module_updated`, fired by `reloadModule` after the
//   accept) re-renders the route with the fresh module — the pre-refresh
//   behavior. Unlike React's fallback it must NOT `location.reload()`: the
//   replay already applies the edit.
//
// And the registry the boot gate consults (`markInPlaceRefresh` /
// `wasRefreshedInPlace`): a URL the framework refreshed IN PLACE (the Vue
// compile step's emission marks it) excludes that module from the boot
// replay — the replay would remount the route and destroy the state the
// framework refresh preserved. Bounded like every other HMR map (§3.8a).

import test from 'node:test'
import assert from 'node:assert/strict'

type HmrWithRefresh = {
  markInPlaceRefresh(url: string): void
  wasRefreshedInPlace(url: string): boolean
  readonly MAX_URLS: number
  __debug: { readonly MAX_URLS: number }
}

const globals = globalThis as unknown as {
  window?: object
  __oxide_hmr__?: HmrWithRefresh
  __oxide_solid_refresh?: (mod: unknown) => void
  $SolidRefreshRuntime$?: { performSolidRefresh?: (mod: unknown) => void } & Record<string, unknown>
}
if (!globals.window) globals.window = globalThis

// Imported AFTER window is set so `install()` runs.
await import('../src/client/hot.ts')

test('hot.ts installs the __oxide_solid_refresh shim', () => {
  assert.equal(typeof globals.__oxide_solid_refresh, 'function')
})

test('the shim delegates to $SolidRefreshRuntime$.performSolidRefresh when present', () => {
  const calls: unknown[] = []
  const g = globalThis as unknown as { $SolidRefreshRuntime$?: { performSolidRefresh?: (m: unknown) => void } }
  g.$SolidRefreshRuntime$ = { performSolidRefresh: (m: unknown) => void calls.push(m) }
  try {
    globals.__oxide_solid_refresh!({ default: { name: 'Comp' } })
    assert.equal(calls.length, 1)
    assert.deepEqual(calls[0], { default: { name: 'Comp' } })
  } finally {
    delete g.$SolidRefreshRuntime$
  }
})

test('a throwing performSolidRefresh must not break the accept callback', () => {
  const g = globalThis as unknown as { $SolidRefreshRuntime$?: { performSolidRefresh?: (m: unknown) => void } }
  g.$SolidRefreshRuntime$ = { performSolidRefresh: () => { throw new Error('refresh blew up') } }
  try {
    assert.doesNotThrow(() => globals.__oxide_solid_refresh!({}))
  } finally {
    delete g.$SolidRefreshRuntime$
  }
})

test('without a runtime the shim neither reloads nor marks in place (the boot replay applies the edit)', () => {
  const g = globalThis as unknown as { $SolidRefreshRuntime$?: unknown; location?: { reload?: () => void } }
  const reloaded: number[] = []
  g.location = { reload: () => void reloaded.push(1) }
  delete g.$SolidRefreshRuntime$
  try {
    const h = globals.__oxide_hmr__!
    h.markInPlaceRefresh('/src/Counter.tsx')
    globals.__oxide_solid_refresh!({})
    // No location.reload happened...
    assert.equal(reloaded.length, 0, 'the Solid shim must never fall back to a full reload')
    // ...and nothing was marked by the shim itself (the markInPlaceRefresh
    // above was the pin's own setup, the shim added nothing for another URL).
    assert.ok(!h.wasRefreshedInPlace('/src/Other.tsx'))
  } finally {
    delete g.location
  }
})

test('markInPlaceRefresh / wasRefreshedInPlace round-trips per URL', () => {
  const h = globals.__oxide_hmr__!
  assert.equal(h.wasRefreshedInPlace('/src/a.vue'), false)
  h.markInPlaceRefresh('/src/a.vue')
  assert.equal(h.wasRefreshedInPlace('/src/a.vue'), true)
  assert.equal(h.wasRefreshedInPlace('/src/b.vue'), false, 'the mark is per-URL')
})

test('the in-place registry is bounded (§3.8a)', () => {
  const h = globals.__oxide_hmr__!
  const MAX = h.__debug.MAX_URLS
  for (let i = 0; i < MAX + 10; i++) {
    h.markInPlaceRefresh(`/synthetic/in-place-${i}`)
  }
  // The oldest entries evicted; the most recent still answer.
  assert.equal(h.wasRefreshedInPlace('/synthetic/in-place-0'), false)
  assert.equal(h.wasRefreshedInPlace(`/synthetic/in-place-${MAX + 9}`), true)
})
