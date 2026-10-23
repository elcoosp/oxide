// §16.6 "Other built-ins": the optional PWA/service-worker module, end to
// end in a browser.
//
// The SW has been emitted (and its precache fixed to include the offline
// fallback) since the r9 work, but nothing exercised it in a real browser —
// r10 carried that gap through several rounds. This spec is the missing
// browser-level test: registration, the precache contents, and an offline
// navigation served from cache.

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
    throw new Error(`oxide-cli binary missing at ${oxideBin}; run \`cargo build -p oxide-cli\` first`)
  }
  // A SW is origin-scoped and persistent; give this spec its own port so a
  // registration cannot collide with another spec's server.
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3107'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3107/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('the service worker registers and its precache carries the shell', async ({ page }) => {
  await page.goto('http://127.0.0.1:3107/')

  // The boot entry appends `/pwa-register.js`; wait for the registration to
  // settle rather than racing it.
  const scope = await page.evaluate(async () => {
    const reg = await navigator.serviceWorker.ready
    return reg.scope
  })
  expect(scope).toContain('127.0.0.1:3107')

  // Inspect the cache the worker populated on install. `PRECACHE` names the
  // shell — the boot entry plus `/` — so an offline navigation has something
  // to serve.
  const cached = await page.evaluate(async () => {
    const names = await caches.keys()
    const out: string[] = []
    for (const n of names) {
      const c = await caches.open(n)
      for (const req of await c.keys()) out.push(new URL(req.url).pathname)
    }
    return out
  })
  expect(cached, `cached paths: ${cached.join(', ')}`).toContain('/')
  expect(cached.some((p) => p.startsWith('/client/'))).toBe(true)
})

test('an offline navigation is served from the precached fallback', async ({ page, context }) => {
  await page.goto('http://127.0.0.1:3107/')
  // Wait for the SW to control this page — a registration that is not yet
  // controlling cannot intercept the offline navigation.
  await page.evaluate(async () => {
    const reg = await navigator.serviceWorker.ready
    if (!reg.active) throw new Error('no active worker')
    if (!navigator.serviceWorker.controller) {
      await new Promise<void>((r) => {
        navigator.serviceWorker.addEventListener('controllerchange', () => r(), { once: true })
      })
    }
  })

  // Go offline and navigate to a path the precache does not hold. The SW's
  // fetch handler falls back to OFFLINE_FALLBACK, which degrades to `/`
  // when the project has no `/offline` route — the smoke app has none.
  await context.setOffline(true)
  const resp = await page.goto('http://127.0.0.1:3107/a-path-not-precached')
  expect(resp, 'offline navigation returns a response from the SW').not.toBeNull()
  // The shell is the precached `/`, so the document carries the app root.
  await expect(page.locator('#root')).toHaveCount(1, { timeout: 5_000 })
  await context.setOffline(false)
})
