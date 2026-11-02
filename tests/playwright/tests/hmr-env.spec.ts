// §3.4: `import.meta.env` in the native-ESM dev server.
//
// Vite parity:
//   - `PUBLIC_*`-prefixed variables from `.env*` files are exposed to
//     client code via `import.meta.env`.
//   - Everything else stays server-side (secrets must not leak).
//   - Editing a `.env*` file triggers a FULL RELOAD (not HMR — env values
//     are read once per module evaluation; letting a hot update through
//     would leave modules that already ran with stale values, silently
//     divergent from a fresh page load).
//   - `import.meta.env.MODE` is `"development"` in dev.
//
// Runs on its own server (port 3114) because the test mutates `.env` and
// expects a specific pre/post state that other tests would race.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3114
const ENV_FILE = join(smokeRoot, '.env')

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined
let savedEnv: string | null = null

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)

  // Save any pre-existing .env so we can restore it after the test.
  if (existsSync(ENV_FILE)) {
    savedEnv = readFileSync(ENV_FILE, 'utf8')
  }
  writeFileSync(
    ENV_FILE,
    'PUBLIC_API_URL=https://before.test/api\nSECRET_INTERNAL=must-not-leak\n',
    'utf8',
  )

  const logTail: string[] = []
  const pushLog = (c: Buffer): void => {
    for (const line of c.toString('utf8').split('\n')) if (line) logTail.push(line)
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }
  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'info' },
  })
  server.stdout?.on('data', pushLog)
  server.stderr?.on('data', pushLog)

  let exited = false
  let info = ''
  server.once('exit', (code, signal) => { exited = true; info = `code=${code} signal=${signal}` })

  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (exited) throw new Error(`oxide dev exited (${info}). Last 50:\n${logTail.slice(-50).join('\n')}`)
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) { void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(10000) }).catch(() => {}); return }
    } catch { /* not up */ }
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch {}
  throw new Error(`oxide dev not serving within 30s. Last 50:\n${logTail.slice(-50).join('\n')}`)
})

test.afterAll(async () => {
  // Restore the pre-test .env state.
  try {
    if (savedEnv !== null) writeFileSync(ENV_FILE, savedEnv, 'utf8')
    else if (existsSync(ENV_FILE)) unlinkSync(ENV_FILE)
  } catch { /* nothing to do */ }

  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test('import.meta.env exposes PUBLIC_* vars, hides SECRET_*, refreshes on .env change (Tier 3.4)', async ({ page, request }) => {
  test.setTimeout(60_000)

  // [1] Served module contains the preamble with PUBLIC_* + MODE, NOT the secret.
  const mod = await request.get('/src/routes/index/page.tsx')
  expect(mod.status()).toBe(200)
  const body = await mod.text()
  expect(body, 'preamble must populate import.meta.env').toContain('import.meta.env')
  expect(body, 'PUBLIC_* vars must be in the preamble').toContain('PUBLIC_API_URL')
  expect(body, 'MODE must be set (Vite parity)').toContain('"MODE":"development"')
  expect(body, 'SECRET_* vars must NOT leak').not.toContain('SECRET_INTERNAL')
  expect(body, 'value must be the pre-change value').toContain('https://before.test/api')

  // [2] Live page: a real browser must load the page without errors. The
  //     served module body isn't directly introspectable via page.evaluate
  //     (import.meta is per-module-context; the dynamic import we'd do from
  //     page.evaluate gets the PAGE's import.meta, not the served module's).
  //     The HTTP-level check in [1] and the poll in [3] are the contract.
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )

  // [3] `.env` edit → served value changes after the reload.
  writeFileSync(ENV_FILE, 'PUBLIC_API_URL=https://after.test/api\nSECRET_INTERNAL=must-not-leak\n', 'utf8')

  await expect.poll(
    async () => {
      const r = await request.get('/src/routes/index/page.tsx')
      if (r.status() !== 200) return null
      const t = await r.text()
      return t.includes('https://after.test/api')
    },
    { timeout: 15_000 },
  ).toBe(true)

  // Secret still must not leak with the new content.
  const after = await request.get('/src/routes/index/page.tsx')
  expect(await after.text()).not.toContain('SECRET_INTERNAL')
})
