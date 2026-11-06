// Vue HMR end-to-end: the ESM dev server previously had NO Vue client story
// — the boot entry hard-coded React's clientAdapter, the manifest never
// carried `.vue` component URLs, and `.vue` files were not served to the
// browser at all. This suite pins the full Vue flow:
//
//   1. Structural (HTTP-only): the manifest carries the `.vue` component
//      URL; the boot entry imports Vue's `clientAdapter`; the served
//      `/src/*.vue` module is pool-compiled JS importing the prebundled
//      `vue` runtime.
//   2. Browser: the page hydrates (Vue `createSSRApp().mount` against the
//      SSR markup) with no module-load errors.
//   3. HMR: editing the `.vue` component updates the DOM in place —
//      `performance.timeOrigin` constant (no navigation) — across THREE
//      successive edits.
//   4. HMR: editing the page loader (`page.server.ts`) re-renders fresh
//      data in place (route-data-update → ox:data → adapter.update).
//
// Fixture: examples/smoke-app-vue. Run:
//   pnpm --filter @oxide/playwright exec playwright test hmr-vue.spec.ts --project=chromium

import { test, expect } from '@playwright/test'
import {
  bootOxideDevHmr,
  captureErrors,
  expectInPlaceUpdate,
  waitHydratedByCounter,
} from './hmr-boot.ts'

const PORT = 3123

// Hardcoded pristine bytes — never captured from disk (a SIGKILLed prior
// run could have left a marker state; capturing from disk would bake the
// pollution into the baseline).
const PRISTINE_TARGET = `<script setup lang="ts">
import { ref } from 'vue'

// HMR fixture: the e2e suite rewrites the marker text in place and watches
// the DOM update without a navigation. The counter pins the remount
// semantics of component-level HMR (state resets, DOM updates in place).
const count = ref(0)
</script>

<template>
  <section>
    <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
    <button data-hmr-counter="" type="button" @click="count++">{{ count }}</button>
  </section>
</template>
`

const PRISTINE_LOADER = `import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (vue)' }
})
`

const hmr = bootOxideDevHmr({
  port: PORT,
  app: 'smoke-app-vue',
  fixtures: [
    { rel: 'src/components/hmr-target.vue', pristine: PRISTINE_TARGET },
    { rel: 'src/routes/index/page.server.ts', pristine: PRISTINE_LOADER },
  ],
})

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

test.describe('Vue ESM dev invariants', () => {
  test('esm manifest carries the .vue component URL', async ({ request }) => {
    const r = await request.get('/@esm-manifest.json')
    expect(r.status()).toBe(200)
    const manifest = (await r.json()) as { routes: Record<string, { component?: string }> }
    const comp = manifest.routes['routes/index/page']?.component
    expect(comp, 'index route must resolve a component').toBeTruthy()
    expect(comp).toBe('/src/routes/index/page.vue')
  })

  test('boot entry wires the Vue clientAdapter', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    expect(r.status()).toBe(200)
    const boot = await r.text()
    // The pre-fix entry imported `clientAdapter` from @oxide-fw/react only,
    // leaving `__adapter` undefined for Vue — no hydration, no HMR socket.
    expect(boot).toContain('clientAdapter')
    expect(boot).toMatch(/from "[^"]*@oxide-fw\/vue\//)
    expect(boot).not.toMatch(/from "[^"]*@oxide-fw\/react\//)
    expect(boot).toContain('bootWithRouter')
  })

  test('served .vue module is pool-compiled JS (no raw SFC)', async ({ request }) => {
    const r = await request.get('/src/routes/index/page.vue')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // The raw SFC (<template>, <script setup>) must never reach the
    // browser: it compiles through @vue/compiler-sfc into a component
    // module importing the (prebundled) vue runtime.
    expect(js).not.toContain('<template>')
    expect(js).toMatch(/from\s+"\/@prebundle\/vue\.js"/)
    // HMR boundary: the preamble assigns import.meta.hot (self-accept —
    // the in-place update is driven by the boot runtime's re-render hook).
    expect(js).toContain('import.meta.hot')
  })
})

test.describe('Vue HMR runtime (browser)', () => {
  test('page hydrates without module-load errors', async ({ page }) => {
    const errors = captureErrors(page)
    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Home', { timeout: 10_000 })
    await expect(page.locator('[data-hmr-target]')).toHaveText('HMR-TARGET-ORIGINAL')
    // Hydration ran: the boot runtime + adapter are installed.
    const booted = await page.evaluate(() => ({
      hmr: typeof (globalThis as { __oxide_hmr__?: unknown }).__oxide_hmr__ === 'object',
      adapter: typeof (globalThis as { __oxideBoot?: { adapter?: unknown } }).__oxideBoot?.adapter === 'object',
    }))
    expect(booted.hmr, 'HMR runtime must be installed').toBe(true)
    expect(booted.adapter, 'clientAdapter must be registered').toBe(true)
    expect(errors, `console errors: ${errors.join(' | ')}`).toEqual([])
  })

  test('HMR websocket connects and receives the build id', async ({ page }) => {
    const frames: string[] = []
    let hmrSocket = false
    page.on('websocket', (ws) => {
      if (!ws.url().includes('/_oxide/hmr')) return
      hmrSocket = true
      ws.on('framereceived', (f) => {
        frames.push(String(f.payload))
      })
    })
    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Home', { timeout: 10_000 })
    await expect
      .poll(() => hmrSocket, { timeout: 10_000, message: 'no /_oxide/hmr websocket' })
      .toBe(true)
    await expect
      .poll(() => frames.some((f) => f.includes('"connected"')), {
        timeout: 10_000,
        message: `no connected frame; got: ${frames.join(' | ')}`,
      })
      .toBe(true)
  })

  test('editing the component updates the DOM in place (no navigation)', async ({ page }) => {
    const errors = captureErrors(page)
    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Home', { timeout: 10_000 })
    await waitHydratedByCounter(page)
    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    // Three successive edits — one edit can pass by luck (a full reload
    // also updates the text); only the no-navigation invariant across a
    // chain of edits proves real HMR.
    for (const text of ['HMR-TARGET-EDIT-1', 'HMR-TARGET-EDIT-2', 'HMR-TARGET-EDIT-3']) {
      hmr.edit('src/components/hmr-target.vue', PRISTINE_TARGET.replace('HMR-TARGET-ORIGINAL', text))
      await expectInPlaceUpdate(page, '[data-hmr-target]', text, timeOrigin)
      // Component-level HMR remounts: the counter state resets. Asserting
      // the reset pins the documented remount semantics.
      await expect(page.locator('[data-hmr-counter]')).toHaveText('0', { timeout: 10_000 })
    }
    expect(errors, `console errors during HMR: ${errors.join(' | ')}`).toEqual([])
  })

  test('editing the loader re-renders fresh data in place', async ({ page }) => {
    const errors = captureErrors(page)
    await page.goto('/')
    await expect(page.locator('main p')).toHaveText('Hello from Oxide (vue)', { timeout: 10_000 })
    await waitHydratedByCounter(page)
    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    hmr.edit(
      'src/routes/index/page.server.ts',
      PRISTINE_LOADER.replace('Hello from Oxide (vue)', 'LOADER-EDITED-VUE'),
    )
    await expectInPlaceUpdate(page, 'main p', 'LOADER-EDITED-VUE', timeOrigin)
    expect(errors, `console errors during loader HMR: ${errors.join(' | ')}`).toEqual([])
  })
})
