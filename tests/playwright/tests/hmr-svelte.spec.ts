// Svelte HMR end-to-end: pins the Svelte flow through the ESM dev server.
//
// Svelte specifics this suite also covers (indirectly, by passing):
//   - `.svelte` files are pool-compiled for the CLIENT with
//     `svelte/compiler --generate dom` (the SSR loader compiles ssr mode
//     separately; before this suite existed the browser could not load a
//     `.svelte` module at all).
//   - The compiled module's `svelte/internal/client` imports resolve to
//     prebundle URLs (framework_prebundle_extras adds them to the rewrite
//     map).
//
// Fixture: examples/smoke-app-svelte (Svelte 5, runes syntax). Run:
//   pnpm --filter @oxide/playwright exec playwright test hmr-svelte.spec.ts --project=chromium

import { test, expect } from '@playwright/test'
import {
  bootOxideDevHmr,
  captureErrors,
  expectInPlaceUpdate,
  waitHydratedByCounter,
} from './hmr-boot.ts'

const PORT = 3125

// Hardcoded pristine bytes — never captured from disk.
const PRISTINE_TARGET = `<script>
  // HMR fixture: the e2e suite rewrites the marker text in place and watches
  // the DOM update without a navigation. The counter pins the remount
  // semantics of component-level HMR (state resets, DOM updates in place).
  let count = $state(0)
</script>

<section>
  <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
  <button data-hmr-counter="" type="button" onclick={() => (count += 1)}>{count}</button>
</section>
`

const PRISTINE_LOADER = `import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (svelte)' }
})
`

const hmr = bootOxideDevHmr({
  port: PORT,
  app: 'smoke-app-svelte',
  fixtures: [
    { rel: 'src/components/hmr-target.svelte', pristine: PRISTINE_TARGET },
    { rel: 'src/routes/index/page.server.ts', pristine: PRISTINE_LOADER },
  ],
})

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

test.describe('Svelte ESM dev invariants', () => {
  test('esm manifest carries the .svelte component URL', async ({ request }) => {
    const r = await request.get('/@esm-manifest.json')
    expect(r.status()).toBe(200)
    const manifest = (await r.json()) as { routes: Record<string, { component?: string }> }
    const comp = manifest.routes['routes/index/page']?.component
    expect(comp, 'index route must resolve a component').toBeTruthy()
    expect(comp).toBe('/src/routes/index/page.svelte')
  })

  test('boot entry wires the Svelte clientAdapter', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    expect(r.status()).toBe(200)
    const boot = await r.text()
    expect(boot).toContain('clientAdapter')
    expect(boot).toMatch(/from "[^"]*@oxide-fw\/svelte\//)
    expect(boot).not.toMatch(/from "[^"]*@oxide-fw\/react\//)
    expect(boot).toContain('bootWithRouter')
  })

  test('served .svelte module is dom-compiled with prebundle imports', async ({ request }) => {
    const r = await request.get('/src/routes/index/page.svelte')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // generate:'dom' output: the runtime pieces come from
    // svelte/internal/client, mapped to the prebundle. Raw SFC markup
    // never reaches the browser. (Svelte's own emitted imports are
    // single-quoted, hence the quote-agnostic match.)
    expect(js).toMatch(/from\s+['"]\/@prebundle\/svelte-internal-client\.js['"]/)
    expect(js).not.toContain('<template>')
    expect(js).not.toMatch(/<section>/)
    expect(js).toContain('import.meta.hot')
  })
})

test.describe('Svelte HMR runtime (browser)', () => {
  test('page hydrates without module-load errors', async ({ page }) => {
    const errors = captureErrors(page)
    await page.goto('/')
    await expect(page.locator('h1')).toHaveText('Home', { timeout: 10_000 })
    await expect(page.locator('[data-hmr-target]')).toHaveText('HMR-TARGET-ORIGINAL')
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

    for (const text of ['HMR-TARGET-EDIT-1', 'HMR-TARGET-EDIT-2', 'HMR-TARGET-EDIT-3']) {
      hmr.edit('src/components/hmr-target.svelte', PRISTINE_TARGET.replace('HMR-TARGET-ORIGINAL', text))
      await expectInPlaceUpdate(page, '[data-hmr-target]', text, timeOrigin)
      // Remount semantics: the counter state resets. No re-hydration wait
      // needed — the next edit's assertions do not click.
      await expect(page.locator('[data-hmr-counter]')).toHaveText('0', { timeout: 10_000 })
    }
    expect(errors, `console errors during HMR: ${errors.join(' | ')}`).toEqual([])
  })

  test('editing the loader re-renders fresh data in place', async ({ page }) => {
    const errors = captureErrors(page)
    await page.goto('/')
    await expect(page.locator('main p')).toHaveText('Hello from Oxide (svelte)', { timeout: 10_000 })
    await waitHydratedByCounter(page)
    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    hmr.edit(
      'src/routes/index/page.server.ts',
      PRISTINE_LOADER.replace('Hello from Oxide (svelte)', 'LOADER-EDITED-SVELTE'),
    )
    await expectInPlaceUpdate(page, 'main p', 'LOADER-EDITED-SVELTE', timeOrigin)
    expect(errors, `console errors during loader HMR: ${errors.join(' | ')}`).toEqual([])
  })
})
