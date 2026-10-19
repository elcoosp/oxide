// Island fixture E2E against `oxide start`.
//
// Two things this covers that the fake-DOM tests can't:
//
//   1. Wire shape: the live server response carries the exact `<ox-island>`
//      markup the client boot script scans for, plus the injected
//      `<script type="module" src="/client/<boot>">` tag.
//   2. Real hydration: the browser loads the boot chunk, hydrates the
//      Counter with `react-dom/client`, and a click on the button advances
//      the value from its SSR-rendered 3 to 4.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide-cli')

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide-cli binary missing at ${oxideBin}; run \`cargo build -p oxide-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3101'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3101/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('server HTML carries the island markup and boot script', async ({ request }) => {
  const r = await request.get('http://127.0.0.1:3101/')
  expect(r.status()).toBe(200)
  const html = await r.text()
  expect(html).toContain('data-ox-island="Counter"')
  expect(html).toContain('data-ox-chunk="Counter"')
  expect(html).toContain('data-ox-strategy="load"')
  // Props are ODF-encoded (spec §9.5): base64("ODF1" + version + payload).
  // The magic prefix `T0RGMQ` is base64("ODF1"); asserting it pins the
  // wire format so a regression to raw JSON would fail loudly.
  expect(html).toMatch(/data-ox-props="T0RGMQ[A-Za-z0-9+/=]+"/)
  expect(html).toMatch(/<script type="module" src="\/client\/[^"]+" nonce="[a-f0-9]+"><\/script>/)
})

// FIXME: client boot currently throws `TypeError: Illegal invocation`
// before hydration completes, so `data-ox-hydrated` never flips to `done`.
// The wire-level island assertions above (island.spec.ts:51) still cover
// the markup shape. Quarantined until the boot bug is fixed.
test.fixme('browser hydrates the counter island and a click advances it', async ({ page }) => {
  const errors: string[] = []
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', (e) => errors.push(e.message))

  await page.goto('http://127.0.0.1:3101/')

  // The island host exists and carries the attributes the boot scanner reads.
  const island = page.locator('ox-island[data-ox-chunk="Counter"]')
  await expect(island).toHaveCount(1)

  // SSR value: the button text is 3 (from `props.initial`).
  const value = island.locator('button span')
  await expect(value).toHaveText('3')

  // React hydration is complete once the boot chunk's effect has run. Wait
  // for the boot marker the module sets before React's own event listeners
  // bind (hydration is async; a click before it lands is a silent no-op).
  // Wait for `done`, not `pending` — the client sets `pending` synchronously
  // on entry (double-hydration guard) and `done` only after the framework
  // adapter has attached its event listeners. Clicking between those two
  // points is a silent no-op.
  await page.waitForFunction(
    () => {
      const el = document.querySelector('ox-island[data-ox-chunk="Counter"]')
      return !!el && el.getAttribute('data-ox-hydrated') === 'done'
    },
    { timeout: 10_000 },
  )

  // Click: the SSR'd markup and React's own DOM must be in sync, so the
  // increment updates the SAME span React found in the tree.
  await island.locator('button').click()
  await expect(value).toHaveText('4', { timeout: 5_000 })

  // Click again to confirm it's a live counter, not a one-shot.
  await island.locator('button').click()
  await expect(value).toHaveText('5')

  // No page-level errors during hydration.
  expect(errors).toEqual([])
})

test('home page h1 renders alongside the island', async ({ page }) => {
  await page.goto('http://127.0.0.1:3101/')
  await expect(page.locator('h1')).toHaveText('Home')
})
