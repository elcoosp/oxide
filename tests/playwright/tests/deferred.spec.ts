// §7.1 / B2: deferred settles are delivered one at a time over a real HTTP
// connection, and the browser ends up with both resolution scripts in the
// DOM. The smoke app's /deferred loader resolves one value at ~80ms and one
// at ~900ms, so the gap is observable without timing tricks.
import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const base = 'http://127.0.0.1:3109'

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(60_000)
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3109'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`${base}/_oxide/health`)
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

test('per-settle scripts arrive before the document closes, then land in the DOM', async ({ page }) => {
  const started = Date.now()
  const response = await fetch(`${base}/deferred`)
  expect(response.status).toBe(200)
  expect(response.body).not.toBeNull()

  const reader = response.body!.getReader()
  const decoder = new TextDecoder()
  let body = ''
  let firstChunkAt = 0
  let fastAt = 0
  let slowAt = 0
  let closeAt = 0

  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    const at = Date.now() - started
    const chunk = decoder.decode(value, { stream: true })
    body += chunk
    if (firstChunkAt === 0) firstChunkAt = at
    if (fastAt === 0 && chunk.includes('data-id="1"')) fastAt = at
    if (slowAt === 0 && chunk.includes('data-id="2"')) slowAt = at
    if (closeAt === 0 && chunk.includes('</body></html>')) closeAt = at
  }

  // The shell is not held for the slow settle, and the fast resolution does
  // not wait for the slow one.
  expect(firstChunkAt).toBeGreaterThan(0)
  expect(firstChunkAt).toBeLessThan(700)
  expect(fastAt).toBeGreaterThan(0)
  expect(slowAt).toBeGreaterThan(0)
  expect(slowAt - fastAt).toBeGreaterThan(300)
  expect(closeAt).toBeGreaterThanOrEqual(slowAt)

  const fast = body.indexOf('data-id="1"')
  const slow = body.indexOf('data-id="2"')
  expect(fast).toBeGreaterThan(0)
  expect(fast).toBeLessThan(slow)
  expect(body.indexOf('<main data-deferred-page="">')).toBeLessThan(fast)
  expect(body.trimEnd().endsWith('</body></html>')).toBe(true)
  expect(body).toContain('"label":"fast"')

  // Browser-level half: both resolution scripts survive parsing and are
  // visible to the client deferred glue.
  await page.goto(`${base}/deferred`)
  const ids = await page
    .locator('script[type="application/oxide-deferred"]')
    .evaluateAll((els) => els.map((e) => e.getAttribute('data-id')))
  expect(ids).toEqual(['1', '2'])
})
