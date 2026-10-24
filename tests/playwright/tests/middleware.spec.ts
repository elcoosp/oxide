// §8.1 middleware.ts / §8.5: real-browser E2E over `oxide start`.
//
// Proves three things the Rust unit tests can't:
//   1. The middleware file is *scanned and dispatched* by the live server
//      (not just by the test harness that injects a fake dispatcher).
//   2. Its injected `locals` reach the real loader through the real worker
//      (the §8.4 parity assertion `greeting` proves it).
//   3. Its `redirect` short-circuits the pipeline (307, `location: /login`)
//      — the `?gated=1` branch the smoke app wires for exactly this test.

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

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3102'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3102/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('middleware injects `locals.user` and the loader reads it', async ({ request }) => {
  // `routes/greeting/page.server.ts` returns `hello, <locals.user>` and
  // `role: <locals.role>`, and `routes/middleware.ts` injects
  // `{ user: 'ada', role: 'admin' }`. If the middleware didn't run — or its
  // locals didn't reach the loader — the page shows `hello, guest` / `anon`.
  const r = await request.get('http://127.0.0.1:3102/greeting')
  expect(r.status()).toBe(200)
  const html = await r.text()
  expect(html).toContain('hello, ada')
  expect(html).toContain('admin')
  expect(html).not.toContain('hello, guest')
})

test('middleware `redirect` short-circuits with 307 + location', async ({ request }) => {
  // The smoke app's middleware returns `{ redirect: '/login', status: 307 }`
  // for `?gated=1`. The pipeline must surface that as a real 307 with the
  // location header set — not as a body, not as a 200.
  const r = await request.get('http://127.0.0.1:3102/greeting?gated=1', { maxRedirects: 0 })
  expect(r.status()).toBe(307)
  expect(r.headers()['location']).toBe('/login')
})

test('browser renders the middleware-fed greeting', async ({ page }) => {
  const errors: string[] = []
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', (e) => errors.push(e.message))

  const r = await page.goto('http://127.0.0.1:3102/greeting')
  expect(r?.status()).toBe(200)
  await expect(page.locator('[data-greeting-text]')).toHaveText('hello, ada')
  await expect(page.locator('[data-greeting-role]')).toHaveText('admin')
  expect(errors).toEqual([])
})
