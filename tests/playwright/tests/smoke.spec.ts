// Real-browser E2E against `oxide start`. The suite is the CLI-level
// counterweight to the fake-DOM hydration tests in packages/{react,vue,...}
// — it exercises the wire contract (HTTP status, headers, body shape) and
// the browser-side rendering path over a real TCP connection.
//
// Islands hydration is scoped out for now: the smoke-app has no `<ox-island>`
// directive, so there is nothing to hydrate. That assertion needs an island
// fixture and lands in a follow-up slice.

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
  // The test assumes `cargo build -p oxide-cli` has been run at least once
  // (dev convenience) and refuses with a clear message otherwise.
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide-cli binary missing at ${oxideBin}; run \`cargo build -p oxide-cli\` first`,
    )
  }

  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3100'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })

  // Wait for the server to accept connections.
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3100/_oxide/health')
      if (r.ok) return
    } catch {
      // not up yet
    }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('GET / returns 200 with the SSR-rendered `<h1>Home</h1>`', async ({ request }) => {
  const r = await request.get('/')
  expect(r.status()).toBe(200)
  const html = await r.text()
  // The smoke-app's index page renders exactly this from source.
  expect(html).toContain('<h1>Home</h1>')
})

test('GET /_oxide/health returns 200 with a small JSON body', async ({ request }) => {
  const r = await request.get('/_oxide/health')
  expect(r.status()).toBe(200)
  expect(Number(r.headers()['content-length'])).toBeGreaterThan(0)
})

test('browser renders the same HTML the server sent', async ({ page }) => {
  const consoleErrors: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text())
  })
  page.on('pageerror', (e) => consoleErrors.push(e.message))

  const r = await page.goto('/')
  expect(r?.status()).toBe(200)
  // The `h1` is present in the live DOM, not just the wire bytes — proves
  // the transport served valid HTML the browser would parse.
  try {
    await expect(page.locator('h1')).toHaveText('Home', { timeout: 5_000 })
  } catch (e) {
    const dump = await page.content().catch(() => '<dump failed>')
    console.error('[browser h1 FAILED] url =', page.url())
    console.error('[browser h1 FAILED] html =', dump.slice(0, 2000))
    console.error('[browser h1 FAILED] consoleErrors =', consoleErrors)
    throw e
  }
  // No page errors during the initial load (no missing chunks, no parse
  // failures). This is the specific thing the fake DOM can't catch.
  expect(consoleErrors).toEqual([])
})

test('dynamic route /blog/hello-prod renders through the loader', async ({ page }) => {
  const r = await page.goto('/blog/hello-prod')
  expect(r?.status()).toBe(200)
  // The blog page renders its title from `props.data.title`, which the
  // loader supplies — so a working loader + render pipeline is what puts
  // this text on screen.
  await expect(page.locator('article h1')).toContainText('hello-prod')
})
