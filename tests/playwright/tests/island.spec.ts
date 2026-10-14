// Island fixture E2E. The smoke-app home page SSRs a `<ox-island>` host for
// the Counter component via `react-dom/server` (through the real adapter).
// The suite asserts the wire HTML carries the island shape the client boot
// script (`packages/oxide/src/client/boot.ts`) scans for.
//
// What this proves today:
//   - `oxide start` (real-SSR path) emits the `<ox-island>` wrapper with
//     the exact attribute names the render crate's `island_element()` uses.
//   - The island's React children render as normal SSR markup inside.
//   - Props survive as base64 in `data-ox-props`.
//
// What it does NOT yet prove (follow-up slices):
//   - Automatic island wrapping (pages currently hand-write the wrapper).
//   - Client-side hydration — clicking the button and observing a DOM
//     change needs the boot script emitted + injected by the build.
//
// Both are documented in WORKLOG session 10.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app')
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

test('home page SSRs an <ox-island> host with the expected attributes', async ({ page }) => {
  await page.goto('http://127.0.0.1:3101/')
  const island = page.locator('ox-island')
  await expect(island).toHaveCount(1)
  await expect(island).toHaveAttribute('data-ox-island', 'Counter')
  await expect(island).toHaveAttribute('data-ox-chunk', '/islands/counter.ts')
  await expect(island).toHaveAttribute('data-ox-strategy', 'load')
  // base64(JSON.stringify({initial:3}))
  await expect(island).toHaveAttribute('data-ox-props', 'eyJpbml0aWFsIjozfQ==')
})

test('island SSR markup contains the counter with the initial value', async ({ page }) => {
  await page.goto('http://127.0.0.1:3101/')
  // React's `renderToString` produced the Counter's button; the value 3
  // comes from `props.initial` carried through the island directive.
  const span = page.locator('ox-island button span')
  await expect(span).toHaveText('3')
})

test('the h1 still renders alongside the island', async ({ page }) => {
  await page.goto('http://127.0.0.1:3101/')
  await expect(page.locator('h1')).toHaveText('Home')
})
