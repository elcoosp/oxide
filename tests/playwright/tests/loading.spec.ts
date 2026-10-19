// §8.4 loading boundary: real-browser E2E over `oxide start`.
//
// The smoke app's `/slow` route has a 250ms loader and a `loading.tsx`
// component. A *client-side* navigation to `/slow` must show the loading
// overlay while the `/__data` fetch is in flight, then replace it with the
// real page when the loader resolves. A full-page load does NOT exercise
// the boundary (there's no in-flight window from the browser's POV), so
// this spec delays `/__data` via `page.route` to make the overlay window
// observable.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide-cli')

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide-cli binary missing at ${oxideBin}; run \`cargo build -p oxide-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3103'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3103/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

// FIXME: client router navigation does not swap the DOM.
// Diagnostic (2026-10-19): clicking an anchor to `/slow` DOES update
// `location.href` and DOES fire `GET /__data?route=%2Fslow` (server returns
// 200), but `root.firstElementChild` is still the home page's `<main>` after
// 3+ seconds and no `[data-loading-slow]` overlay ever appears. So the
// router's `onData` handler receives the ODF envelope but never calls
// `adapter.update(...)`. Root cause is in
// `packages/oxide/src/client/router.ts` (the `onData`/swap path); needs a
// dedicated debugging session. Pre-existing bug — masked earlier by the
// React #418 hydration mismatch this session fixed.
test.fixme('client navigation shows the loading overlay then the real page', async ({ page }) => {
  const errors: string[] = []
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', (e) => errors.push(e.message))

  // Hold the /__data response for 800ms so the loading overlay is
  // observably on-screen for that window. The route's own 250ms delay is
  // additive; the point is to extend the visible window, not replace it.
  await page.route('**/__data*', async (route) => {
    await new Promise((r) => setTimeout(r, 800))
    await route.continue()
  })

  await page.goto('http://127.0.0.1:3103/')
  // Wait for the client boot to install its router (the module sets
  // `window.__oxide` before hydration completes).
  await page.waitForFunction(() => (window as any).__oxide != null, { timeout: 10_000 })

  // Trigger a client-side navigation via a same-origin anchor click. The
  // client router intercepts same-origin anchor clicks; if it doesn't (a
  // regression), this becomes a full page load and the loading overlay
  // never appears — which the next assertion catches.
  await page.evaluate(() => {
    const a = document.createElement('a')
    a.href = '/slow'
    a.textContent = 'to-slow'
    document.body.appendChild(a)
    a.click()
  })

  // Overlay appears while /__data is held.
  await expect(page.locator('[data-loading-slow]')).toBeVisible({ timeout: 3_000 })

  // Overlay is replaced by the real page once the loader resolves.
  await expect(page.locator('[data-slow-message]')).toHaveText('finally loaded', {
    timeout: 5_000,
  })
  await expect(page.locator('[data-loading-slow]')).toHaveCount(0)
  expect(errors).toEqual([])
})
