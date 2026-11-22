// §V2-R01 C26 — stale build 409 + x-oxide-build-stale (A04).
//
// An unknown build id on the data plane answers 409 with
// `x-oxide-build-stale: true`; the HTML plane is never the data plane (a
// bogus id on `/` still answers 200). Mirrors the skew arc's unknown-id step
// against the smoke app. CI `playwright` job.
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
const base = 'http://127.0.0.1:3212'

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3212'], {
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

test('an unknown build id 409s on /__data with x-oxide-build-stale', async ({ request }) => {
  // An unknown id on the data plane → 409 + the stale marker.
  const data = await request.get(`${base}/__data?route=/`, {
    headers: { 'x-oxide-build': '0123456789abcdef' },
    maxRedirects: 0,
  })
  expect(data.status()).toBe(409)
  expect(data.headers()['x-oxide-build-stale']).toBeTruthy()

  // The HTML plane is never the data plane: a bogus id on `/` answers 200.
  const html = await request.get(`${base}/`, {
    headers: { 'x-oxide-build': '0123456789abcdef' },
    maxRedirects: 0,
  })
  expect(html.status()).toBe(200)
})
