// §9.3 / §22: form actions work with JavaScript disabled.
//
// The spec's form-first contract: `<form method='post' action='?/comment'>`
// works without client JS — the server validates, runs the action, and
// re-renders the page with the result in `data.form`. `enhance` is the
// JS-on upgrade; this spec is the JS-off baseline, which §22 names as a
// required run.
//
// No SW registration interference: this context never loads the app with JS,
// so nothing else is in flight.

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
    throw new Error(`oxide-cli binary missing at ${oxideBin}; run \`cargo build -p oxide-cli\` first`)
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3108'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3108/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('a form submission runs the action and re-renders with its result', async ({ browser }) => {
  // §22: the no-JS run. A context with JS disabled is the only way to prove
  // the form path does not secretly depend on client hydration.
  const context = await browser.newContext({ javaScriptEnabled: false })
  const page = await context.newPage()

  await page.goto('http://127.0.0.1:3108/comment')
  // The form renders server-side; with JS off there is no hydration.
  await expect(page.locator('form[action="?/comment"]')).toHaveCount(1)
  await expect(page.locator('[data-comment-result]')).toHaveCount(0)

  // A real browser form submission — no fetch, no JS.
  await page.locator('input[name="text"]').fill('from-no-js')
  await page.locator('button[type="submit"]').click()

  // The server ran the action and re-rendered this page with `data.form`.
  await expect(page.locator('[data-comment-result]')).toHaveText(/Received: from-no-js/, {
    timeout: 10_000,
  })
  // The form is still there, ready for another submission.
  await expect(page.locator('form[action="?/comment"]')).toHaveCount(1)

  await context.close()
})

test('the action also works over a direct POST (curl-shaped)', async ({ request }) => {
  const r = await request.post('http://127.0.0.1:3108/comment?/comment', {
    form: { text: 'from-post' },
  })
  expect(r.status()).toBe(200)
  // §9.3: a successful action re-renders, so the response is HTML, not JSON.
  expect(r.headers()['content-type']).toContain('text/html')
  // React's SSR inserts a comment separator between adjacent text nodes
  // (`Received: <!-- -->from-post`), so match the payload, not the literal
  // contiguous string.
  const html = await r.text()
  expect(html).toContain('data-comment-result')
  expect(html).toMatch(/Received:\s*(<!-- -->)?from-post/)
})
