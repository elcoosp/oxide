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
const PORT = 3122
const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))
const UTIL = join(smokeRoot, 'src', 'lib', 'util.ts')

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0, timeout: 60_000 })

let server: ChildProcess | undefined
let pristine = ''

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  pristine = readFileSync(UTIL, 'utf8')
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
  server.once('exit', () => { exited = true })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (exited) throw new Error(`oxide dev exited early`)
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
  try { writeFileSync(UTIL, pristine, 'utf8') } catch {}
  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((r) => server!.once('exit', () => r()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test.beforeEach(async () => {
  let cur = ''
  try { cur = readFileSync(UTIL, 'utf8') } catch { cur = '' }
  if (cur !== pristine) { writeFileSync(UTIL, pristine, 'utf8'); await sleep(300) }
})
test.afterEach(async () => {
  let cur = ''
  try { cur = readFileSync(UTIL, 'utf8') } catch { cur = '' }
  if (cur !== pristine) writeFileSync(UTIL, pristine, 'utf8')
  await sleep(2000)
})

test('HMR: leaf util edit updates importer without reload (Tier 1.3)', async ({ page }) => {
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed')
  const marker = `UTIL-CHANGED-${Date.now()}`

  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  await sleep(500)

  const utilSpan = page.locator('[data-hmr-util]')
  const counter = page.locator('button[data-hmr-counter]')
  await expect(utilSpan).toHaveText('UTIL-ORIGINAL')
  await expect(counter).toHaveText('0')

  await counter.click()
  await expect(counter).toHaveText('1', { timeout: 5000 })

  const timeOrigin = await page.evaluate(() => performance.timeOrigin)

  const patched = pristine.replace('UTIL-ORIGINAL', marker)
  if (patched === pristine) throw new Error('util.ts lost UTIL-ORIGINAL marker')
  writeFileSync(UTIL, patched, 'utf8')

  await expect(utilSpan).toHaveText(marker, { timeout: 20_000 })
  await expect(counter).toHaveText('1', { timeout: 5000 })
  expect(
    await page.evaluate(() => performance.timeOrigin),
    '§1.3: leaf util edit must not reload the page',
  ).toBe(timeOrigin)
})
