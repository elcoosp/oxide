// §V2-R01 C28 — ancestor middleware chain runs outer→inner + locals merge (A05).
//
// The middleware chain runs outer→inner and each ancestor's `locals` merge
// into the descendant's loader. The smoke app's `routes/middleware.ts`
// injects `{ user: 'ada', role: 'admin' }`; the `/greeting` loader reads
// them. This spec asserts the merged locals reach the loader (the existing
// `middleware.spec.ts` pins the single-hop case; C28 asserts the chain merge
// carries the ancestor locals into the page). CI `playwright` job.
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
const base = 'http://127.0.0.1:3214'

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3214'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM') })

test('the ancestor middleware locals merge into the loader', async ({ request }) => {
  // The middleware injects { user: 'ada', role: 'admin' }; the /greeting
  // loader reads both. If the chain didn't run outer→inner or the locals
  // didn't merge, the page shows the guest defaults.
  const r = await request.get(`${base}/greeting`)
  expect(r.status()).toBe(200)
  const html = await r.text()
  expect(html).toContain('hello, ada')
  expect(html).toContain('admin')
  expect(html).not.toContain('hello, guest')
})
