// §1.2: $RefreshSig$ real implementation.
//
// React Refresh's second guarantee (after in-place update) is detecting
// SHAPE changes that require a remount: hook count, hook order, or a
// custom-hook dependency change. Without a real $RefreshSig$, React
// Refresh treats every version of a component as "the same shape" and
// does an in-place update — silently keeping the OLD closures against
// the new hook chain. That's the class of "component updated but
// behaviour is wrong" bug §1.2 exists to close.
//
// The compiler now emits, for every top-level hook call inside a
// component:
//
//     _s(useState, "useState")(0)
//
// where `_s = $RefreshSig$()` and the runtime's
// `createSignatureFunctionForTransform()` records the hook name. When the
// hook sequence changes (a hook is added, removed, or reordered), the
// runtime computes a different `fullKey` — `performReactRefresh()`
// remounts the component instead of patching it.
//
// This test asserts that observable contract:
//
//   1. Load fixture with ONE hook (useState). Click 3 times → state = 3.
//   2. Edit the fixture to add a SECOND hook (useEffect). Hook sequence
//      changes from ["useState"] to ["useState", "useEffect"].
//   3. Assert state RESETS to 0 (remount happened). If §1.2 regressed,
//      state stays 3 — in-place update ran against the wrong shape.
//
// Distinct from §2.4's test (which asserts state PRESERVED on an edit that
// does NOT change the hook sequence). This one asserts state RESET on an
// edit that DOES.

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
const PORT = 3120

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))
const FIXTURE = join(smokeRoot, 'src', 'components', 'hmr-shape-target.tsx')

const PRISTINE = `import { useState } from 'react'\n\n// §1.2 acceptance fixture. Deliberately minimal: ONE hook (useState), then\n// the test edits the file to add a SECOND hook (useEffect). Before §1.2's\n// real $RefreshSig$, React Refresh could not detect this shape change, so\n// it would do an in-place update and PRESERVE the counter (state stays 3\n// after 3 clicks). With $RefreshSig$ recording the hook sequence, the\n// runtime sees "useState → useState, useEffect" and REMOUNTS — counter\n// resets to 0.\nexport default function HmrShapeTarget() {\n  const [count, setCount] = useState(0)\n  return (\n    <>\n      <p data-shape-target="">SHAPE-ORIGINAL</p>\n      <button data-shape-counter="" onClick={() => setCount((c) => c + 1)}>{count}</button>\n    </>\n  )\n}\n`

// The edited version adds `useEffect` AFTER `useState` — hook order in the
// runtime's `fullKey` is ["useState", "useEffect"]. Different from the
// pristine's ["useState"], so React Refresh must remount.
const PATCHED = `import { useEffect, useState } from 'react'\n\nexport default function HmrShapeTarget() {\n  const [count, setCount] = useState(0)\n  useEffect(() => { /* shape-changing hook added */ }, [])\n  return (\n    <>\n      <p data-shape-target="">SHAPE-PATCHED</p>\n      <button data-shape-counter="" onClick={() => setCount((c) => c + 1)}>{count}</button>\n    </>\n  )\n}\n`

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0, timeout: 60_000 })

let server: ChildProcess | undefined

test.beforeAll(async () => {
  // §1.2: extend this hook's own timeout — `test.describe.configure({ timeout })`
  // above sets the TEST timeout but Playwright's beforeAll/afterAll hooks use
  // their own budget (default 30s, or config default when the file's suite
  // configures it). Cold `oxide dev` boot after a `packages/core` change
  // rebuilds the prebundle, which can exceed 15s on a first run. SIGTERM on
  // hook timeout leaves orphan worker children holding the test port and
  // poisons the next project in the matrix. 60s cleanly covers cold boot.
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)

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
  test.setTimeout(30_000)
  try { writeFileSync(FIXTURE, PRISTINE, 'utf8') } catch { /* nothing */ }
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
  if (cur !== PRISTINE) {
    writeFileSync(FIXTURE, PRISTINE, 'utf8')
    await sleep(300)
  }
})
test.afterEach(async () => {
  let cur = ''
  try { cur = readFileSync(FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== PRISTINE) writeFileSync(FIXTURE, PRISTINE, 'utf8')
  await sleep(2000)
})

test('adding a hook forces remount, not stale in-place update (Tier 1.2)', async ({ page }) => {
  test.setTimeout(60_000)
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

  try {
    await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
    await page.waitForFunction(
      () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
      { timeout: 20_000 },
    )
    await sleep(500)

    const counter = page.locator('button[data-shape-counter]')
    await expect(counter).toHaveText('0')
    // Settle post-hydration on Firefox. `__oxideBoot` is set synchronously
    // in boot.ts, but React's `hydrateRoot` attachment (which wires the
    // onClick handler) can complete a tick later. Chromium and WebKit finish
    // before the first Playwright click; Firefox occasionally doesn't, so
    // the first click lands on a handler-less button. 200ms is well above
    // observed variance and doesn't affect test runtime meaningfully.
    await page.waitForTimeout(200)
    // Three clicks with an intermediate assertion after each + a small
    // settle. Without the settle, firefox occasionally loses the second
    // click — the intermediate `toHaveText` poll sees the state update
    // from click 1, but React's `hydrateRoot` handler-swap for the click-2
    // dispatch races the DOM commit. Same category as §2.1's island
    // hardening; chromium and webkit tolerate the absence, firefox needs
    // the explicit micro-settle.
    // Click via `element.click()` in page.evaluate rather than
    // `locator.click()`. Playwright's pointer simulation on firefox has
    // known flakiness against a button whose React onClick handler is
    // attached via `hydrateRoot` (the synthetic-event delegation
    // occasionally misses the second pointer sequence). `element.click()`
    // fires the exact same DOM click event React handles — same semantics,
    // reliable delivery across all three engines. Chromium and WebKit
    // tolerate `locator.click()`; firefox does not.
    const domClick = async (): Promise<void> => {
      await page.evaluate(() => {
        const btn = document.querySelector('button[data-shape-counter]')
        ;(btn as HTMLButtonElement | null)?.click()
      })
    }
    await domClick()
    await expect(counter).toHaveText('1', { timeout: 5000 })
    await domClick()
    await expect(counter).toHaveText('2', { timeout: 5000 })
    await domClick()
    await expect(counter).toHaveText('3', { timeout: 5000 })

    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    // Edit: add `useEffect` AFTER `useState`. Same first hook, but the
    // sequence is now ["useState", "useEffect"] vs ["useState"] — a
    // signature mismatch the runtime must act on.
    writeFileSync(FIXTURE, PATCHED, 'utf8')

    // The target text changes (SHAPE-ORIGINAL → SHAPE-PATCHED) — proves the
    // edit propagated. Either outcome (remount OR in-place) would show the
    // new text; the counter assertion below is what discriminates.
    await expect(page.locator('p[data-shape-target]')).toHaveText('SHAPE-PATCHED', {
      timeout: 15_000,
    })

    // THE ASSERTION: state must have RESET (remount), not stayed at 3
    // (in-place against a mismatched shape). Before §1.2, this test would
    // see '3' — the exact failure it exists to catch.
    await expect(
      counter,
      `§1.2: adding a hook must force a REMOUNT (counter resets to 0). ` +
      `Seeing '3' here means $RefreshSig$ is not recording the hook sequence, ` +
      `so React Refresh did an in-place update against a shape that does not match. ` +
      `This is the "component updated but behaviour is wrong" bug §1.2 exists to close.`,
    ).toHaveText('0')

    // No page reload — remount happens via React Refresh, not a refresh.
    expect(
      await page.evaluate(() => performance.timeOrigin),
      'remount must not reload the page',
    ).toBe(timeOrigin)
  } finally {
    writeFileSync(FIXTURE, PRISTINE, 'utf8')
  }
})
