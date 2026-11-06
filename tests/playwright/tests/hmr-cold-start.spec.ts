// Tier 2.13: cold-start HMR race (U6 in docs/plan-esm-hmr.md).
//
// The race: during page boot, `$RefreshReg$` is called synchronously from
// every component module as it evaluates. At that instant `$RefreshRuntime$`
// has not finished loading — hot.ts pulls it in via a dynamic
// `import('/@prebundle/react-refresh-runtime.js')`. Without buffering, every
// initial registration is silently dropped (the runtime wasn't there to
// receive it). Later, an edit's fresh module re-registers the component;
// without a baseline, React Refresh treats it as a brand-new component and
// `performReactRefresh()` finds nothing to correlate against. The DOM never
// updates in place.
//
// `pendingRegistrations` in hot.ts buffers the initial calls and drains them
// into `rt.register()` once the runtime loads. This spec is the end-to-end
// proof that the buffering works when the edit lands WITHIN 500ms of
// `__oxideBoot` — the tightest window a real user can hit.
//
// Why this is its OWN spec file (not a test in `hmr.spec.ts`):
//   The HMR suite in `hmr.spec.ts` shares one `oxide dev` server across all
//   its tests. The layout test (Tier 2.2) triggers `RestartSubsystems`,
//   which rescans the whole route tree and regenerates the ESM manifest +
//   boot entry. The next test's `page.goto('/')` can race that rebuild on
//   a busy machine and burn the 60s test budget waiting for a response.
//   Verified empirically: this test passes in isolation (4.8s) but times
//   out at 60s when run after the layout test in the same describe.
//   A dedicated spec file gets its own server, its own lifecycle, and its
//   own fixture guard — no shared state, no cross-test contamination.
//   Same architecture that §2.7 (WS reconnect) will need, for the same
//   reason.
//
// Run: `pnpm --filter @oxide/playwright exec playwright test
//       hmr-cold-start.spec.ts --project=chromium --workers=1 --retries=0`

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
const PORT = 3110

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\``)
  }

  const logTail: string[] = []
  const pushLog = (chunk: Buffer): void => {
    for (const line of chunk.toString('utf8').split('\n')) {
      if (line.length > 0) logTail.push(line)
    }
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }

  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  server.stdout?.on('data', pushLog)
  server.stderr?.on('data', pushLog)

  let exited = false
  let exitInfo = ''
  server.on('exit', (code, signal) => {
    exited = true
    exitInfo = `code=${code} signal=${signal}`
  })

  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    if (exited) {
      throw new Error(
        `oxide dev exited before serving /_oxide/health (${exitInfo}). Last 50 log lines:\n` +
        logTail.slice(-50).join('\n'),
      )
    }
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        // Warm the SSR path so the first test's `goto('/')` doesn't pay
        // the cold-compile cost against its own 30s budget.
        void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(5000) }).catch(() => {})
        return
      }
    } catch { /* server not up yet */ }
    await sleep(250)
  }
  throw new Error(
    `oxide dev did not serve /_oxide/health within 15s. Last 50 log lines:\n` +
    logTail.slice(-50).join('\n'),
  )
})

test.afterAll(async () => {
  // Wait for the process to ACTUALLY exit before returning. `server.kill()`
  // is fire-and-forget: it sends SIGTERM and returns immediately, but the
  // process can keep running for seconds while it drains. That matters here
  // because the NEXT spec in the run (hmr.spec.ts) spawns its own dev
  // server, and both briefly coexist watching examples/smoke-app-react/src.
  // Two watchers on the same tree, writing the same .oxide/ cache, is a
  // real source of cross-spec state corruption — see the earlier handoff's
  // fixture-contamination note. Await the `exit` event so the next spec
  // starts against a clean process tree.
  if (!server) return
  const exited = new Promise<void>((resolve) => {
    if (server!.exitCode !== null || server!.signalCode !== null) {
      resolve()
      return
    }
    server!.once('exit', () => resolve())
  })
  server.kill('SIGTERM')
  const forceKill = setTimeout(() => {
    try { server!.kill('SIGKILL') } catch { /* already gone */ }
  }, 5000)
  await exited
  clearTimeout(forceKill)
})

// Fixture guard: identical contract to the hmr.spec.ts guard, but scoped
// to this spec's fixture set (only `hmr-target.tsx`). Pristine bytes are
// hardcoded because the file is NOT git-tracked (per the earlier handoff).
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

test('HMR: cold-start edit within 500ms of __oxideBoot applies without reload (Tier 2.13)', async ({ page }) => {
  test.setTimeout(60_000)
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

  const original = HMR_PRISTINE
  const marker1 = `COLD1-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  const marker2 = `COLD2-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

  try {
    await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })

    // Tight polling (Playwright default is rAF ~16ms; 50ms keeps the
    // detection window small without busy-waiting). The point is to reach
    // the edit write fast enough that the delta stays under 500ms.
    await page.waitForFunction(
      () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
      { timeout: 20_000, polling: 50 },
    )
    const bootAt = Date.now()
    const timeOrigin = await page.evaluate(() => performance.timeOrigin)

    // Write IMMEDIATELY — no settle sleep. That is the whole test.
    const patched1 = original.replace(/HMR-TARGET-ORIGINAL/, marker1)
    if (patched1 === original) throw new Error('hmr-target.tsx lost the HMR-TARGET-ORIGINAL marker')
    writeFileSync(HMR_FIXTURE, patched1, 'utf8')
    const delta = Date.now() - bootAt
    expect(
      delta,
      `cold-start edit landed ${delta}ms after __oxideBoot; §2.13's window is <500ms. ` +
      `If this fires, the machine was too slow to exercise the race — do not widen ` +
      `the assertion without updating the plan, because the test's purpose is to hit ` +
      `this window.`,
    ).toBeLessThan(500)

    // Observable effect of the registration replay: in-place update.
    await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker1, { timeout: 15_000 })
    expect(
      await page.evaluate(() => performance.timeOrigin),
      'cold-start edit must apply in place, not reload',
    ).toBe(timeOrigin)

    // Post-cold-start runtime health: after a replayed-then-edited
    // component, further edits must still work in place. A buffering bug
    // that happens to survive one edit but corrupts the family mapping
    // would pass the first assertion and fail this one.
    await new Promise((r) => setTimeout(r, 500))
    const patched2 = original.replace(/HMR-TARGET-ORIGINAL/, marker2)
    writeFileSync(HMR_FIXTURE, patched2, 'utf8')
    await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker2, { timeout: 15_000 })
    expect(
      await page.evaluate(() => performance.timeOrigin),
      'follow-up edit after a cold-start update must also apply in place',
    ).toBe(timeOrigin)
  } finally {
    writeFileSync(HMR_FIXTURE, original, 'utf8')
    await new Promise((r) => setTimeout(r, 1500))
  }
  })

test('cold-start: first page render within budget (Tier 2.15)', async ({ page }) => {
  test.setTimeout(30_000)

  // §2.15: cold-start bound — sanity ceiling on how long a user waits for a
  // fresh page in `oxide dev`. The plan says "<5s on local hardware". Two
  // measurements, both useful:
  //
  //   1. Wall-clock from `page.goto('/')` to `window.__oxideBoot` being set.
  //      What a real user perceives between typing the URL and the page
  //      being interactive enough to run client code.
  //   2. Browser-native DOMContentLoaded, from
  //      `PerformanceNavigationTiming` (NOT the deprecated
  //      `performance.timing` the plan mentions). Independent of
  //      Playwright's polling overhead — catches a regression that lands
  //      between DCL and `__oxideBoot` (e.g. a slow prebundle import).
  //
  // Budget: 5s on local hardware, 15s on CI (shared runners are slower and
  // the plan explicitly scopes to local hardware). Normal runs land at
  // 1-3s. A hit here means something is badly wrong, not slightly slow.
  //
  // No fixture mutation — this test only reads. Runs on all three engines
  // (§2.14's invariant).
  const budgetMs = process.env.CI ? 15_000 : 5_000

  const startedAt = Date.now()
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 10_000, polling: 50 },
  )
  const wallClock = Date.now() - startedAt

  const nav = await page.evaluate(() => {
    const entries = performance.getEntriesByType('navigation')
    const n = entries[0] as PerformanceNavigationTiming | undefined
    if (!n) return null
    return {
      dcl: n.domContentLoadedEventEnd - n.fetchStart,
      load: n.loadEventEnd > 0 ? n.loadEventEnd - n.fetchStart : null,
    }
  })

  // Log to test output — useful when reviewing a passing run's timings.
  // eslint-disable-next-line no-console
  console.log(`[2.15] wallClock=${wallClock}ms dcl=${nav?.dcl?.toFixed(0) ?? '?'}ms budget=${budgetMs}ms`)

  expect(
    wallClock,
    `cold-start wall-clock ${wallClock}ms exceeded budget ${budgetMs}ms. ` +
    `Regression class: slow route scan, prebundle not cached, or SSR that ` +
    `blocks DCL. Browser nav timing: ${JSON.stringify(nav)}`,
  ).toBeLessThan(budgetMs)

  if (nav && nav.dcl > 0) {
    expect(
      nav.dcl,
      `DOMContentLoaded ${nav.dcl.toFixed(0)}ms exceeded budget ${budgetMs}ms`,
    ).toBeLessThan(budgetMs)
  }
})

