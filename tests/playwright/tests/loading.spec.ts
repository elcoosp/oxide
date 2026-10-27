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
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')

let server: ChildProcess | undefined

test.beforeAll(async ({ browser }) => {
  // beforeAll can spend longer than the file's 15s test timeout on the
  // server-up poll, the warm-up navigation, or both; give it room.
  test.setTimeout(60_000)

  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3103'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  let up = false
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3103/_oxide/health')
      if (r.ok) { up = true; break }
    } catch { /* not up */ }
    await sleep(250)
  }
  if (!up) throw new Error('oxide start did not come up within 30s')

  // Warm-up navigation. The first `page.goto` + `waitForFunction(__oxide)`
  // in this file has been observed to time out on WebKit's very first
  // run (cold engine + cold server on the test's 10s budget), then pass
  // on retry in ~2s once both are warm. A throw-away navigation here
  // pays both costs once, outside any test's timeout, so the test below
  // starts from a warm engine and a warm server.
  const warmup = await browser.newPage()
  try {
    await warmup.goto('http://127.0.0.1:3103/')
    await warmup.waitForFunction(() => (window as any).__oxide != null, {
      timeout: 30_000,
    })
  } finally {
    await warmup.close()
  }
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('client navigation shows the loading overlay then the real page', async ({ page }) => {
  const errors: string[] = []
  // Capture ALL console output, not just errors. The real-mount fallback path
  // logs `oxide: real page render failed; falling back to IR` as a WARNING,
  // which the previous error-only filter hid — so a mount failure surfaced as a
  // bare 'element(s) not found' with no diagnostic. Capturing warnings makes a
  // future mount failure actionable at the point it happens.
  page.on('console', (m) => {
    errors.push(`[${m.type()}] ${m.text()}`)
  })
  page.on('pageerror', (e) => errors.push('PAGEERROR: ' + e.message))

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
  //
  // The real mount is ASYNC: onNavigateEnd tears down the overlay synchronously,
  // then adapter.update kicks off an async chain (componentFor -> chunk import
  // -> mountReactPageRoot). In slower environments (CI) that chain can take
  // hundreds of milliseconds; a 5s budget was too tight — it timed out at
  // 'element(s) not found' with no signal why. 15s gives headroom for a slow
  // chunk import while still failing on a genuine stall. On failure the error
  // carries the page state and the last 20 console lines, so the next CI run
  // reports WHY mount did not complete instead of just 'not found'.
  try {
    await expect(page.locator('[data-slow-message]')).toHaveText('finally loaded', {
      timeout: 15_000,
    })
  } catch (e) {
    const body = await page.content()
    const state = await page.evaluate(() => ({
      overlayCount: document.querySelectorAll('[data-loading-slow]').length,
      slowCount: document.querySelectorAll('[data-slow]').length,
      notfoundCount: document.querySelectorAll('[data-ox-not-found]').length,
      rootHtml: (document.getElementById('root') as HTMLElement | null)?.innerHTML?.slice(0, 500) ?? '',
    }))
    throw new Error(
      '[data-slow-message] did not appear within 15s.\n' +
      'state: ' + JSON.stringify(state, null, 2) + '\n' +
      'console (' + errors.length + '): ' + JSON.stringify(errors.slice(-20), null, 2) + '\n' +
      'body (first 2000):\n' + body.slice(0, 2000),
    )
  }
  await expect(page.locator('[data-loading-slow]')).toHaveCount(0)
  // Warnings are allowed (deprecations, framework info); FAIL only on errors.
  const realErrors = errors.filter((e) => e.startsWith('[error]') || e.startsWith('PAGEERROR:'))
  expect(realErrors).toEqual([])
})
