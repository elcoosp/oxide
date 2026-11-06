// §2.9: `oxide dev --no-esm` — the bundler dev path (dev.esm: false).
//
// The native ESM server is the default (§13.3). The bundler path predates
// it and is still live: `ensure_dev_client_bundle` runs at dev startup
// when `!cfg.dev.esm`, produces `dist/client/client.manifest.json`, and the
// SSR shell serves `/client/__boot__.js` from that manifest instead of the
// ESM boot entry `/@boot/boot.mjs`.
//
// `--no-esm` is the flag that reaches this path without editing the config
// file. It overrides `cfg.dev.esm` in `cmd_dev` AND threads an env var
// through to `start_server_inner_full`'s config reload (which would
// otherwise discard the override).
//
// This spec runs its own server (port 3112) — a `--no-esm` server serves a
// fundamentally different boot graph; sharing a port with the ESM specs
// would be confusing at best, wrong at worst.

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
const PORT = 3112

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(`oxide binary missing at ${oxideBin}`)
  }
  const logTail: string[] = []
  const pushLog = (c: Buffer): void => {
    for (const line of c.toString('utf8').split('\n')) if (line) logTail.push(line)
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }
  server = spawn(
    oxideBin,
    ['dev', '--root', smokeRoot, '--port', String(PORT), '--no-esm'],
    { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, RUST_LOG: 'info' } },
  )
  server.stdout?.on('data', pushLog)
  server.stderr?.on('data', pushLog)

  let exited = false
  let info = ''
  server.once('exit', (code, signal) => { exited = true; info = `code=${code} signal=${signal}` })

  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (exited) {
      throw new Error(`oxide dev --no-esm exited (${info}). Last 50 log:\n${logTail.slice(-50).join('\n')}`)
    }
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(10000) }).catch(() => {})
        return
      }
    } catch { /* not up */ }
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch { /* gone */ }
  throw new Error(`oxide dev --no-esm not serving within 30s. Last 50 log:\n${logTail.slice(-50).join('\n')}`)
})

test.afterAll(async () => {
  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test('--no-esm serves /client/__boot__.js, not /@boot/boot.mjs (Tier 2.9)', async ({ request, page }) => {
  test.setTimeout(60_000)

  // 1. HTML references the BUNDLER boot chunk, not the ESM one.
  const r = await request.get('/')
  expect(r.status()).toBe(200)
  const html = await r.text()
  expect(html, 'bundler mode must reference /client/<hash>.js').toContain('/client/__boot__.js')
  expect(html, 'bundler mode must NOT reference the ESM boot entry').not.toContain('/@boot/boot.mjs')
  // The ESM mode always references /@prebundle/ (prebundled CJS deps); the
  // bundler path does not — its boot chunk is one file.
  expect(html, 'bundler mode must NOT reference /@prebundle/').not.toContain('/@prebundle/')

  // 2. The boot chunk is actually fetchable — not just a URL in the HTML.
  const boot = await request.get('/client/__boot__.js')
  expect(boot.status(), '/client/__boot__.js must be served').toBe(200)
  const bootBody = await boot.text()
  expect(bootBody.length, 'boot chunk must be non-trivial (has content)').toBeGreaterThan(1000)

  // 3. Real Chromium: page hydrates (same test as §13.3's
  //    `page hydrates without module-load errors`, but against the
  //    bundler path).
  // §2.9 acceptance: page loads and hydrates. `__oxideBoot` set means the
  // boot chunk executed; `<body>` non-empty means SSR emitted something.
  // Console errors are NOT part of this contract — they belong to §3.11's
  // acceptance (four real bundler-dev bugs, filed there).
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  await expect(page.locator('body')).not.toBeEmpty()

  // §3.11 acceptance: no console errors and no pageerrors on the bundler
  // path. Strict — zero noise filter other than generic browser chrome.
  // If this fails, the failing pattern IS the current §3.11 bug (reconcile
  // against the plan's recon section; add to the plan, do NOT filter).
  const errors: string[] = []
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', (e) => errors.push(e.message))
  // The listeners above attach AFTER the assertions; to actually capture
  // boot-time errors, re-navigate with them in place.
  await page.reload({ waitUntil: 'domcontentloaded' })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  const noise = [/favicon/i, /Datadog/i, /installHook/i]
  const real = errors.filter((e) => !noise.some((re) => re.test(e)))
  expect(real, `console errors on bundler path: ${real.join('\n')}`).toEqual([])
})
