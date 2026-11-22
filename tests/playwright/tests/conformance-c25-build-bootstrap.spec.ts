// §V2-R01 C25 — build bootstrap + x-oxide-build on /__data (A04).
//
// The HTML bootstrap advertises the canonical build id (`window.__oxide.build`)
// and every data-plane `/__data` response carries `x-oxide-build`. Drives the
// live `oxide start` server against the smoke app — the CI `playwright` job
// runs this after `global-setup` builds the smoke app.
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
const base = 'http://127.0.0.1:3211'

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3211'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`${base}/_oxide/health`)
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM') })

test('the HTML bootstrap advertises build: and /__data carries x-oxide-build', async ({ request }) => {
  // The HTML page's bootstrap carries `window.__oxide.build` (the canonical id).
  const html = await request.get(`${base}/`)
  expect(html.status()).toBe(200)
  const body = await html.text()
  expect(body).toMatch(/__oxide[^}]*build["']?\s*:/)

  // Every data-plane /__data response carries x-oxide-build (§V2-A04 step 4).
  const data = await request.get(`${base}/__data?route=/`)
  expect(data.status()).toBe(200)
  const buildHeader = data.headers()['x-oxide-build']
  expect(buildHeader).toBeTruthy()
  expect(buildHeader!.length).toBeGreaterThanOrEqual(16)
})
