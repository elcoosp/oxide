// §3.3: prebundle invalidation on node_modules changes.
//
// The ESM prebundle (`.oxide/esm-prebundle/<pkg>.js`) converts CJS-only
// packages to ESM so the browser can import them via `/@prebundle/<pkg>.js`.
// Before §3.3, it was built once at dev boot: `pnpm add foo` (or `pnpm
// update`ing a bundled dep) left the on-disk prebundle serving the STALE
// conversion forever, and `/@prebundle/<pkg>.js` 404'd for any package
// installed after boot.
//
// §3.3 adds a watcher branch: a `node_modules/*/package.json` event or a
// lockfile event re-runs `prebundle_cjs_deps` and publishes a FullReload
// (client reloads, picks up new prebundle on next request).
//
// Test: boot --no-esm=off (default ESM) server, snapshot the prebundle
// mtime, create a real package.json under node_modules/, assert the mtime
// bumped within a few seconds AND a `full-reload` frame reached a live
// client. Own server (3113) because the test mutates node_modules.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3113
const PREBUNDLE_JS = join(smokeRoot, '.oxide', 'esm-prebundle', 'react.js')
const FAKE_PKG_DIR = join(smokeRoot, 'node_modules', '.oxide-test-prebundle-diag')

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
    if (exited) {
      throw new Error(`oxide dev exited (${info}). Last 50 log:\n${logTail.slice(-50).join('\n')}`)
    }
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        // Warm SSR; the prebundle exists before serving.
        void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(10000) }).catch(() => {})
        return
      }
    } catch { /* not up */ }
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch {}
  throw new Error(`oxide dev not serving within 30s. Last 50 log:\n${logTail.slice(-50).join('\n')}`)
})

test.afterAll(async () => {
  // Always clean up the fake package dir even if the test failed.
  try { rmSync(FAKE_PKG_DIR, { recursive: true, force: true }) } catch {}
  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test('node_modules package.json change regenerates the ESM prebundle (Tier 3.3)', async ({ page }) => {
  test.setTimeout(45_000)

  // Precondition: the prebundle exists (built at boot).
  expect(existsSync(PREBUNDLE_JS), `prebundle missing at ${PREBUNDLE_JS}`).toBe(true)
  const mtimeBefore = statSync(PREBUNDLE_JS).mtimeMs

  // Open a page so a FullReload frame has a live socket to reach.
  const frames: string[] = []
  page.on('websocket', (ws) => {
    if (ws.url().includes('/_oxide/hmr')) {
      ws.on('framereceived', (eventOrPayload: unknown) => {
        const raw = eventOrPayload && typeof eventOrPayload === 'object'
          && 'payload' in (eventOrPayload as object)
            ? (eventOrPayload as { payload: unknown }).payload
            : eventOrPayload
        let text: string
        if (typeof raw === 'string') text = raw
        else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw))
          text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
        else if (raw && typeof (raw as { toString?: unknown }).toString === 'function')
          text = (raw as { toString(enc: string): string }).toString('utf8')
        else text = String(raw)
        frames.push(text)
      })
    }
  })
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  await sleep(500)

  // Create a REAL (non-symlink) package.json under node_modules. `pnpm`
  // symlinks most package.json files into the global store, so touching
  // one of those modifies a file OUTSIDE the watch root. A brand-new
  // directory under the app's own node_modules is a real watcher event.
  mkdirSync(FAKE_PKG_DIR, { recursive: true })
  writeFileSync(
    join(FAKE_PKG_DIR, 'package.json'),
    JSON.stringify({ name: 'oxide-test-prebundle-diag', version: '0.0.0' }, null, 2),
    'utf8',
  )

  // Wait for the prebundle mtime to bump (re-pre-bundling happened).
  const mtimeDeadline = Date.now() + 10_000
  let mtimeAfter = mtimeBefore
  while (Date.now() < mtimeDeadline) {
    await sleep(200)
    if (existsSync(PREBUNDLE_JS)) {
      mtimeAfter = statSync(PREBUNDLE_JS).mtimeMs
      if (mtimeAfter > mtimeBefore) break
    }
  }
  expect(
    mtimeAfter,
    `prebundle mtime did not advance; before=${mtimeBefore} after=${mtimeAfter}. ` +
    `The §3.3 invalidation branch in crates/cli/src/lib.rs (has_prebundle_invalidation) ` +
    `did not fire. Check that the fake package.json path matches ` +
    `/node_modules/ + ends_with('/package.json').`,
  ).toBeGreaterThan(mtimeBefore)

  // Wait for a `full-reload` frame to arrive at the open socket.
  await expect.poll(
    () => frames.find((f) => f.includes('"type":"full-reload"')),
    { timeout: 10_000 },
  ).toBeTruthy()
})

// Suppress the "unused import" diagnostic — readFileSync is imported for
// parity with sibling HMR specs but this test doesn't currently read.
void readFileSync
