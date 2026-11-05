// Tier 2.7: WebSocket reconnect after a transient dev-server outage.
//
// The reconnect LOOP has always worked (`onclose` schedules `open` at
// `reconnectMs`). What §2.7 actually tests — and what used to fail — is
// that the page SURVIVES the outage long enough to receive the post-
// reconnect ModuleUpdate. Before §3.10's defensive catches, an
// `ERR_CONNECTION_REFUSED` on `/@prebundle/react-dom-client.js` escaped
// as a page-level rejection and React unmounted the tree; there was
// nothing left to receive the edit.
//
// This spec runs on its OWN server (port 3111) because it kills that
// server mid-test. Sharing a server with `hmr.spec.ts` would break every
// subsequent test in that file.

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
const PORT = 3111

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined

async function spawnServer(): Promise<ChildProcess> {
  const logTail: string[] = []
  const pushLog = (c: Buffer): void => {
    for (const line of c.toString('utf8').split('\n')) if (line) logTail.push(line)
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }
  const s = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  s.stdout?.on('data', pushLog)
  s.stderr?.on('data', pushLog)
  let exited = false
  let info = ''
  s.once('exit', (code, signal) => { exited = true; info = `code=${code} signal=${signal}` })

  const deadline = Date.now() + 20_000
  while (Date.now() < deadline) {
    if (exited) throw new Error(`oxide dev exited (${info}). Last 50 log:\n${logTail.slice(-50).join('\n')}`)
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(5000) }).catch(() => {})
        return s
      }
    } catch { /* not up */ }
    await sleep(200)
  }
  try { s.kill('SIGKILL') } catch { /* gone */ }
  throw new Error(`oxide dev not serving within 20s. Last 50 log:\n${logTail.slice(-50).join('\n')}`)
}

async function killServer(s: ChildProcess | undefined): Promise<void> {
  if (!s) return
  if (s.exitCode !== null || s.signalCode !== null) return
  const exited = new Promise<void>((resolve) => s.once('exit', () => resolve()))
  s.kill('SIGTERM')
  const force = setTimeout(() => { try { s.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
}

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  server = await spawnServer()
})

test.afterAll(async () => {
  await killServer(server)
})

const HMR_FIXTURE = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
const HMR_PRISTINE = `import { useState } from 'react'\nimport { utilValue } from '../lib/util.ts'\n\nexport default function HmrTarget() {\n  const [count, setCount] = useState(0)\n  return (\n    <>\n      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>\n      <span data-hmr-util="">{utilValue}</span>\n      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>\n    </>\n  )\n}\n`

test.beforeEach(async () => {
  let cur = ''
  try { cur = readFileSync(HMR_FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== HMR_PRISTINE) {
    writeFileSync(HMR_FIXTURE, HMR_PRISTINE, 'utf8')
    await new Promise((r) => setTimeout(r, 300))
  }
})
test.afterEach(async () => {
  let cur = ''
  try { cur = readFileSync(HMR_FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== HMR_PRISTINE) writeFileSync(HMR_FIXTURE, HMR_PRISTINE, 'utf8')
  await new Promise((r) => setTimeout(r, 2000))
})

test('HMR: page survives server restart, reconnect applies subsequent edit (Tier 2.7)', async ({ page }) => {
  test.setTimeout(90_000)
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

  const marker = `RECONNECT-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  const pageErrors: string[] = []
  page.on('pageerror', (e) => pageErrors.push(e.message))

  try {
    await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
    await page.waitForFunction(
      () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
      { timeout: 20_000 },
    )
    await new Promise((r) => setTimeout(r, 500))

    const timeOrigin = await page.evaluate(() => performance.timeOrigin)
    await expect(page.locator('h1[data-hmr-target]')).toHaveText('HMR-TARGET-ORIGINAL')

    // Kill the server. The page's WS gets `onclose`, schedules a reconnect.
    // Any in-flight lazy import gets `ERR_CONNECTION_REFUSED`; §3.10's
    // catches prevent those rejections from unmounting anything.
    await killServer(server)
    server = undefined

    // Give the client at least one failed reconnect attempt. Also gives
    // any straggling lazy import (chunk not yet fetched) time to fail
    // against a dead server — that's the exact §3.10 scenario.
    await sleep(2000)

    // THE ASSERTION THAT USED TO FAIL: page is still alive.
    const survived = await page.evaluate(() => ({
      hasBoot: typeof (window as { __oxideBoot?: unknown }).__oxideBoot === 'object',
      h1Count: document.querySelectorAll('h1[data-hmr-target]').length,
      h1Text: document.querySelector('h1[data-hmr-target]')?.textContent ?? null,
    }))
    expect(
      survived.hasBoot,
      `page after outage: ${JSON.stringify({ survived, pageErrors })}. ` +
      `__oxideBoot should still be an object — if it's gone, React unmounted ` +
      `the tree, exactly the §3.10 bug the catches are supposed to prevent.`,
    ).toBe(true)
    expect(survived.h1Count, 'HMR target must still exist in the DOM after the outage').toBe(1)
    expect(survived.h1Text).toBe('HMR-TARGET-ORIGINAL')

    // Restart the server on the same port.
    server = await spawnServer()

    // Give the client's `onclose -> setTimeout(open, 1000)` a couple
    // cycles to reconnect.
    await sleep(2500)

    // Write the edit. If the client's WS reconnected AND the page is alive
    // AND its module graph is intact enough to apply an in-place Refresh,
    // this lands without a reload.
    const patched = HMR_PRISTINE.replace(/HMR-TARGET-ORIGINAL/, marker)
    writeFileSync(HMR_FIXTURE, patched, 'utf8')

    await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker, { timeout: 20_000 })

    // The page never reloaded — same timeOrigin throughout.
    expect(
      await page.evaluate(() => performance.timeOrigin),
      'the edit after reconnect must apply in place, not via a page reload',
    ).toBe(timeOrigin)

    // No uncaught error escaped to the page — that's the §3.10 guarantee.
    // Exclude ERR_CONNECTION_REFUSED: those are expected logs during the
    // outage, and the point is that they no longer surface as page errors.
    const unexpectedErrors = pageErrors.filter(
      (m) => !/ERR_CONNECTION_REFUSED|Failed to fetch dynamically imported module/.test(m),
    )
    expect(
      unexpectedErrors,
      `unexpected page errors during outage: ${unexpectedErrors.join('\n')}`,
    ).toEqual([])
  } finally {
    writeFileSync(HMR_FIXTURE, HMR_PRISTINE, 'utf8')
  }
})
