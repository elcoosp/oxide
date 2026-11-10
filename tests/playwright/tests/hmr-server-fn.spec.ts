// §1.5: editing a plain `.server.ts` body must NOT reload the page.
//
// Before: `classify_path("greeting.server.ts")` → `ServerModule` →
// `ServerSwap` → the v0 wire's `server-module-swap` frame, whose client
// arm shared its shape with `full-reload` → `location.reload()`. So
// editing a server-fn body lost the user's page state for no benefit.
//
// After: same edit still fires `invalidate_framework_modules` on the
// server (worker generation bump → next call re-imports fresh code),
// but the CLIENT no longer reloads. The next user-triggered call gets
// fresh data; existing DOM stays.
//
// Runs on its own server (port 3121) because it mutates
// `greeting.server.ts` and expects the reload to be the discriminator.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3121

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))
const FIXTURE = join(smokeRoot, 'src', 'greeting.server.ts')

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0, timeout: 60_000 })

let server: ChildProcess | undefined
let pristine = ''

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  pristine = readFileSync(FIXTURE, 'utf8')

  const logTail: string[] = []
  const pushLog = (c: Buffer): void => {
    for (const line of c.toString('utf8').split('\n')) if (line) logTail.push(line)
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }
  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
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
    } catch {}
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch {}
  throw new Error('oxide dev not serving within 30s')
})

test.afterAll(async () => {
  try { writeFileSync(FIXTURE, pristine, 'utf8') } catch {}
  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test.beforeEach(async () => {
  let cur = ''
  try { cur = readFileSync(FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== pristine) {
    writeFileSync(FIXTURE, pristine, 'utf8')
    await sleep(300)
  }
})
test.afterEach(async () => {
  let cur = ''
  try { cur = readFileSync(FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== pristine) writeFileSync(FIXTURE, pristine, 'utf8')
  await sleep(2000)
})

test('HMR: plain .server.ts body edit does NOT reload (Tier 1.5)', async ({ page }) => {
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed')

  const frames: string[] = []
  page.on('websocket', (ws) => {
    if (!ws.url().includes('/_oxide/hmr')) return
    ws.on('framereceived', (ev: unknown) => {
      const raw = ev && typeof ev === 'object' && 'payload' in (ev as object)
        ? (ev as { payload: unknown }).payload
        : ev
      let text = ''
      if (typeof raw === 'string') text = raw
      else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw))
        text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
      else text = String(raw)
      frames.push(text)
    })
  })

  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  await sleep(500)

  const timeOrigin = await page.evaluate(() => performance.timeOrigin)
  const framesBefore = frames.length

  // Edit greeting.server.ts body.
  const patched = pristine.replace(
    "greeting: `hello, ${locals?.user ?? 'guest'}`",
    "greeting: `HELLO-TIER-1-5-${locals?.user ?? 'guest'}`",
  )
  if (patched === pristine) {
    // Fallback: append a comment (still triggers the watcher)
    writeFileSync(FIXTURE, pristine + '\n// §1.5 test edit\n', 'utf8')
  } else {
    writeFileSync(FIXTURE, patched, 'utf8')
  }

  // Wait for at least one WS message (the swap’s invalidation frame or similar).
  await expect.poll(
    () => frames.length,
    { timeout: 10_000, intervals: [100, 200, 500] },
  ).toBeGreaterThan(framesBefore)

  // The critical assertion.
  await sleep(1500)
  expect(
    await page.evaluate(() => performance.timeOrigin),
    '§1.5: a plain `.server.ts` body edit must NOT reload. ' +
    'The server already bumped the worker generation; the next call gets ' +
    'fresh code. But the page itself must stay put.',
  ).toBe(timeOrigin)
})
