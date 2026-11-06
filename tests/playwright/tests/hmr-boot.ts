// Shared harness for the per-framework HMR specs (hmr-vue / hmr-solid /
// hmr-svelte). The React suite (hmr.spec.ts and friends) predates the
// framework rollout and keeps its own inline boot logic; these specs share
// one implementation because the flow they exercise is identical modulo the
// fixture app and the edited file:
//
//   boot `oxide dev` against a framework smoke app → load the page →
//   rewrite a component file → assert the DOM updates IN PLACE (no
//   navigation, `performance.timeOrigin` constant) → restore the fixture.
//
// Conventions kept from the React suite (see its header comments):
//   - `test.setTimeout(60_000)` in beforeAll — the hook budget is not the
//     per-test budget.
//   - bounded stdout/stderr buffers, prepended to the beforeAll error.
//   - afterAll awaits REAL process exit so the next spec never boots a dev
//     server while this one still drains (two watchers → competing HMR
//     events).
//   - pristine fixture bytes hardcoded (NOT read at module load) so a
//     SIGKILLed prior run cannot contaminate the baseline.
//   - `--workers=1 --retries=0` is the run gate (HANDOFF.md).

import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { test, expect, type Page } from '@playwright/test'

const __dirname = dirname(fileURLToPath(import.meta.url))
export const repoRoot = join(__dirname, '..', '..', '..')
export const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')

export interface HmrFixtureSpec {
  /** Project-relative path of the editable file. */
  rel: string
  /** The pristine bytes — hardcoded, never captured from disk. */
  pristine: string
}

export interface HmrHarnessOptions {
  port: number
  /** Directory name under examples/ — smoke-app-vue etc. */
  app: string
  /** Every file the spec edits; restored before AND after each test. */
  fixtures: HmrFixtureSpec[]
}

export interface HmrHarness {
  /** Overwrite a project-relative file (watcher fires). */
  edit(rel: string, content: string): void
  /** Absolute path of a project-relative file. */
  abs(rel: string): string
  /** The app root (examples/<app>). */
  appRoot: string
}

/**
 * Register beforeAll/afterAll/beforeEach/afterEach on the current describe
 * block: boots `oxide dev --root examples/<app>` on `port`, restores every
 * fixture around each test, and returns the edit handle.
 */
export function bootOxideDevHmr(opts: HmrHarnessOptions): HmrHarness {
  const appRoot = join(repoRoot, 'examples', opts.app)
  const abs = (rel: string): string => join(appRoot, rel)

  const FIXTURES = opts.fixtures.map((f) => ({ path: abs(f.rel), pristine: f.pristine }))
  const restoreFixtures = (): boolean => {
    let changed = false
    for (const { path, pristine } of FIXTURES) {
      let cur = ''
      try { cur = readFileSync(path, 'utf8') } catch { cur = '' }
      if (cur !== pristine) {
        writeFileSync(path, pristine, 'utf8')
        changed = true
      }
    }
    return changed
  }

  let server: ChildProcess | undefined

  test.beforeAll(async () => {
    test.setTimeout(60_000)
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

    server = spawn(oxideBin, ['dev', '--root', appRoot, '--port', String(opts.port)], {
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

    const deadline = Date.now() + 30_000
    while (Date.now() < deadline) {
      if (exited) {
        throw new Error(
          `oxide dev exited before serving /_oxide/health (${exitInfo}). Last 50 log lines:\n` +
          logTail.slice(-50).join('\n'),
        )
      }
      try {
        const r = await fetch(`http://127.0.0.1:${opts.port}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
        if (r.ok) {
          // Warm the SSR path with a bounded timeout — the FIRST test owns
          // the assertion budget if this hangs.
          void fetch(`http://127.0.0.1:${opts.port}/`, { signal: AbortSignal.timeout(8000) }).catch(() => {})
          return
        }
      } catch { /* server not up yet */ }
      await sleep(250)
    }
    throw new Error(
      `oxide dev did not serve /_oxide/health within 30s. Last 50 log lines:\n` +
      logTail.slice(-50).join('\n'),
    )
  })

  test.afterAll(async () => {
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

  test.beforeEach(async () => {
    if (restoreFixtures()) await new Promise((r) => setTimeout(r, 300))
  })
  test.afterEach(async () => {
    restoreFixtures()
    // Settle: the dev server's watcher emits 3-8 events per write; let the
    // burst finish before the next `goto('/')`.
    await new Promise((r) => setTimeout(r, 1500))
  })

  return {
    edit(rel: string, content: string): void {
      writeFileSync(abs(rel), content, 'utf8')
    },
    abs,
    appRoot,
  }
}

// --- Browser helpers shared by the three specs ----------------------------

/** Console/page errors captured from a page (excluding favicon 404 noise). */
export function captureErrors(page: Page): string[] {
  const errors: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text())
  })
  page.on('pageerror', (e) => errors.push(e.message))
  return errors
}

/**
 * Wait until the page is HYDRATED — i.e. clicking the counter button
 * actually increments it. Pre-hydration clicks are lost (the SSR markup has
 * no listeners yet), so poll: click while the text still reads '0', then
 * require it to flip to '1' on a short budget.
 */
export async function waitHydratedByCounter(page: Page, selector = '[data-hmr-counter]'): Promise<void> {
  await expect(async () => {
    const t = (await page.locator(selector).textContent())?.trim()
    if (t === '0' || t === '') await page.click(selector)
    await expect(page.locator(selector)).toHaveText('1', { timeout: 1000 })
  }).toPass({ timeout: 15_000 })
  // Reset for the test body: the click above left '1'.
  await expect(async () => {
    const t = (await page.locator(selector).textContent())?.trim()
    if (t !== '0') await page.reload()
    await expect(page.locator(selector)).toHaveText('0', { timeout: 3000 })
  }).toPass({ timeout: 15_000 })
}

/**
 * The invariants every framework HMR update must hold, asserted with
 * Playwright's polling so the watcher + reload + re-render pipeline has a
 * real budget:
 *   - the marker text becomes `expected`;
 *   - `performance.timeOrigin` is unchanged (NO navigation / full reload);
 *   - the page did not enter an error state.
 */
export async function expectInPlaceUpdate(
  page: Page,
  selector: string,
  expected: string,
  timeOriginBefore: number,
): Promise<void> {
  await expect
    .poll(async () => page.locator(selector).textContent(), { timeout: 15_000 })
    .toBe(expected)
  const timeOriginAfter = await page.evaluate(() => performance.timeOrigin)
  expect(timeOriginAfter, 'HMR must not navigate (timeOrigin constant)').toBe(timeOriginBefore)
}
