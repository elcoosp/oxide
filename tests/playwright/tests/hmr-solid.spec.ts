// Solid HMR end-to-end: pins the Solid flow through the ESM dev server.
//
// Solid specifics this suite also covers (indirectly, by passing):
//   - SSR on linux-x64: `solid-jsx-oxc` has no linux binary, so the dev
//     SSR loader compiles Solid tsx via the babel-preset-solid fallback.
//   - The client module for a Solid `.tsx` page is pool-compiled with
//     `babel-preset-solid --generate dom` (NOT the React jsx() lowering).
//
// The HMR invariants are the same as every framework: in-place DOM update,
// no navigation, across successive edits; loader edits re-render data.
//
// Fixture: examples/smoke-app-solid. Run:
//   pnpm --filter @oxide/playwright exec playwright test hmr-solid.spec.ts --project=chromium

import { test, expect } from '@playwright/test'
import {
  bootOxideDevHmr,
  captureErrors,
  expectInPlaceUpdate,
  waitHydratedByCounter,
} from './hmr-boot.ts'

const PORT = 3124

// Hardcoded pristine bytes — never captured from disk.
const PRISTINE_TARGET = `import { createSignal } from 'solid-js'

// HMR fixture: the e2e suite rewrites the marker text in place and watches
// the DOM update without a navigation. The counter pins the remount
// semantics of component-level HMR (state resets, DOM updates in place).
export default function HmrTarget() {
  const [count, setCount] = createSignal(0)
  return (
    <section>
      <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
      <button data-hmr-counter="" type="button" onClick={() => setCount(count() + 1)}>
        {count()}
      </button>
    </section>
  )
}
`

const PRISTINE_LOADER = `import { defineLoad } from '@oxide-fw/core/server'

export const load = defineLoad(async () => {
  return { message: 'Hello from Oxide (solid)' }
})
`

const hmr = bootOxideDevHmr({
  port: PORT,
  app: 'smoke-app-solid',
  fixtures: [
    { rel: 'src/components/hmr-target.tsx', pristine: PRISTINE_TARGET },
    { rel: 'src/routes/index/page.server.ts', pristine: PRISTINE_LOADER },
  ],
})

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

test.describe('Solid ESM dev invariants', () => {
  test('esm manifest carries the Solid tsx component URL', async ({ request }) => {
    const r = await request.get('/@esm-manifest.json')
    expect(r.status()).toBe(200)
    const manifest = (await r.json()) as { routes: Record<string, { component?: string }> }
    const comp = manifest.routes['routes/index/page']?.component
    expect(comp, 'index route must resolve a component').toBeTruthy()
    expect(comp).toBe('/src/routes/index/page.tsx')
  })

  test('boot entry wires the Solid clientAdapter', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    expect(r.status()).toBe(200)
    const boot = await r.text()
    expect(boot).toContain('clientAdapter')
    expect(boot).toMatch(/from "[^"]*@oxide-fw\/solid\//)
    expect(boot).not.toMatch(/from "[^"]*@oxide-fw\/react\//)
    expect(boot).toContain('bootWithRouter')
  })

  test('served Solid tsx compiles to solid runtime calls (not react jsx)', async ({ request }) => {
    const r = await request.get('/src/routes/index/page.tsx')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // babel-preset-solid DOM output: template()/insert()/createComponent
    // from the prebundled @solidjs/web. The React jsx() lowering would
    // import /@prebundle/react-jsx-runtime.js — that is the exact
    // "React runtime on a Solid page" regression this guards. (The
    // prebundle URL sanitizer strips '@', hence `solidjs-web.js`.)
    expect(js).toMatch(/from\s+"\/@prebundle\/solidjs-web\.js"/)
    expect(js).toMatch(/template\(|createComponent\(|delegateEvents\(/)
    expect(js).not.toMatch(/react-jsx-runtime|jsxDEV?\(/)
    // No React Refresh emit on a Solid page (the accept is the generic
    // self-accept; __oxide_refresh would full-reload without a runtime).
    expect(js).not.toContain('$RefreshReg$')
    expect(js).toContain('import.meta.hot')
  })
})

test.describe('Solid HMR runtime (browser)', () => {
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
      hmr.edit('src/components/hmr-target.tsx', PRISTINE_TARGET.replace('HMR-TARGET-ORIGINAL', text))
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
    await expect(page.locator('main p')).toHaveText('Hello from Oxide (solid)', { timeout: 10_000 })
    await waitHydratedByCounter(page)
    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    hmr.edit(
      'src/routes/index/page.server.ts',
      PRISTINE_LOADER.replace('Hello from Oxide (solid)', 'LOADER-EDITED-SOLID'),
    )
    await expectInPlaceUpdate(page, 'main p', 'LOADER-EDITED-SOLID', timeOrigin)
    expect(errors, `console errors during loader HMR: ${errors.join(' | ')}`).toEqual([])
  })
})
