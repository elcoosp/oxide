// §V2-R01 C29 — gated route's /__data is gated too (A05).
//
// A route whose middleware gates the HTML pipeline also gates its `/__data`
// (the data plane rides the same middleware chain). The smoke app's
// middleware redirects `?gated=1` to `/login` (307) on the HTML plane; C29
// asserts the data plane `/__data?route=/greeting&gated=1` is gated too (a
// 307 redirect, not a 200 with the loader data). CI `playwright` job.
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
const base = 'http://127.0.0.1:3215'

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3215'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM') })

test("the gated route's /__data is gated too", async ({ request }) => {
  // The HTML plane: ?gated=1 → 307 redirect to /login.
  const html = await request.get(`${base}/greeting?gated=1`, { maxRedirects: 0 })
  expect(html.status()).toBe(307)
  expect(html.headers()['location']).toBe('/login')
  // The data plane rides the same chain: /__data?route=/greeting&gated=1 is
  // gated too (a redirect, not a 200 with the loader's ODF payload).
  const data = await request.get(`${base}/__data?route=/greeting&gated=1`, { maxRedirects: 0 })
  expect(data.status()).toBeGreaterThanOrEqual(300)
  expect(data.status()).toBeLessThan(400)
})
