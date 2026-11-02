// §3.5: `import.meta.hot` inside a Web Worker.
//
// Before this: `install()` in hot.ts bailed on `typeof window ===
// 'undefined'` — so `__oxide_hmr__` was never installed in a Worker
// scope. The dev preamble assigned `import.meta.hot =
// globalThis.__oxide_hmr__.createHotContext(...)`, but the guard above
// meant `__oxide_hmr__` itself didn't exist. Worker code saw
// `import.meta.hot === undefined`.
//
// Fix: `install()` detects worker scope (`typeof importScripts ===
// 'function'`), installs `__oxide_hmr__` there too (minus React Refresh,
// which is DOM-only), and opens a worker-side WS to `/_oxide/hmr`. The
// worker's `reloadModule(url)` re-imports its own module graph; the main
// thread's React Refresh flow is independent.
//
// Test: navigate to /worker, verify the worker posts `hotDefined: true`
// with `WORKER-V0`. Edit the worker source to `WORKER-V1`. Verify the
// worker posts again with `WORKER-V1` — no page reload, same worker
// instance (the accept callback fired with the new module).

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3115
const WORKER_SRC = join(smokeRoot, 'src', 'workers', 'counter.worker.ts')

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined
let workerPristine: string = ''

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  workerPristine = readFileSync(WORKER_SRC, 'utf8')

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
    } catch { /* not up */ }
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch {}
  throw new Error(`oxide dev not serving within 30s. Last 50:\n${logTail.slice(-50).join('\n')}`)
})

test.afterAll(async () => {
  // Always restore the fixture.
  try {
    if (workerPristine) writeFileSync(WORKER_SRC, workerPristine, 'utf8')
  } catch { /* nothing to do */ }
  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test('worker sees import.meta.hot and receives a hot update on edit (Tier 3.5)', async ({ page }) => {
  test.setTimeout(60_000)
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

  const pageErrors: string[] = []
  page.on('pageerror', (e) => pageErrors.push(e.message))

  try {
    await page.goto('/worker', { waitUntil: 'domcontentloaded', timeout: 30_000 })
    await page.waitForFunction(
      () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
      { timeout: 20_000 },
    )

    // [1] Worker posts the initial message with hotDefined=true, WORKER-V0.
    await page.waitForFunction(
      () => {
        const el = document.querySelector('[data-worker-status]')
        const v = el?.getAttribute('data-worker-status') ?? ''
        return v.startsWith('{') && v.includes('WORKER-V0')
      },
      { timeout: 15_000 },
    )
    const before = await page.evaluate(() => {
      const el = document.querySelector('[data-worker-status]')
      const raw = el?.getAttribute('data-worker-status') ?? ''
      try { return JSON.parse(raw) } catch { return null }
    })
    expect(before, 'worker message before edit').not.toBeNull()
    expect(before.hotDefined, '§3.5: import.meta.hot must be defined in the worker').toBe(true)
    expect(before.v).toBe('WORKER-V0')

    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    // [2] Edit the worker source. The dev server publishes a ModuleUpdate
    //     for the worker URL; the WORKER's own WS receives it and calls
    //     reloadModule, which re-imports the module — accept fires, posts
    //     the new message.
    const patched = workerPristine.replace(/WORKER-V0/g, 'WORKER-V1')
    if (patched === workerPristine) throw new Error('worker src lost WORKER-V0')
    writeFileSync(WORKER_SRC, patched, 'utf8')

    // [3] Worker posts the new message.
    await page.waitForFunction(
      () => {
        const el = document.querySelector('[data-worker-status]')
        const v = el?.getAttribute('data-worker-status') ?? ''
        return v.startsWith('{') && v.includes('WORKER-V1')
      },
      { timeout: 15_000 },
    )

    const after = await page.evaluate(() => {
      const el = document.querySelector('[data-worker-status]')
      const raw = el?.getAttribute('data-worker-status') ?? ''
      try { return JSON.parse(raw) } catch { return null }
    })
    expect(after).not.toBeNull()
    expect(after.v).toBe('WORKER-V1')
    expect(after.hotDefined, 'worker still sees import.meta.hot after update').toBe(true)

    // [4] The page never reloaded — worker HMR is in-worker, main thread untouched.
    expect(
      await page.evaluate(() => performance.timeOrigin),
      'worker HMR must not reload the main page',
    ).toBe(timeOrigin)

    // No unexpected page errors (the worker may log benign errors during
    // the re-import; those surface as pageerror only if they escape).
    const unexpected = pageErrors.filter((m) => !/favicon|Datadog|installHook/i.test(m))
    expect(unexpected, `page errors: ${unexpected.join('\n')}`).toEqual([])
  } finally {
    writeFileSync(WORKER_SRC, workerPristine, 'utf8')
  }
})
