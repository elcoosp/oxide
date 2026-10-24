// §9.6: SSE route E2E over `oxide start`.
//
// The smoke app's `routes/api/events/route.ts` returns a `ReadableStream`
// body that emits three `data: tick <n>` events then closes. The Rust
// pipeline must stream the body unbuffered, so a client sees the events
// arrive progressively (not all at once at the end). This spec asserts:
//   1. the wire headers are the SSE contract (`content-type`, `cache-control`);
//   2. the full body carries all three events in order;
//   3. the browser's `fetch` receives them progressively (the body reader
//      resolves more than once, proving no full-body buffering).

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

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3105'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3105/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('GET /api/events streams three `tick N` events with SSE headers', async ({ request }) => {
  const r = await request.get('http://127.0.0.1:3105/api/events')
  expect(r.status()).toBe(200)
  expect(r.headers()['content-type']).toContain('text/event-stream')
  expect(r.headers()['cache-control']).toContain('no-cache')
  const body = await r.text()
  // The route emits exactly: `data: tick 0\n\n`, `data: tick 1\n\n`, `data: tick 2\n\n`.
  const events = body
    .split('\n\n')
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
    .map((s) => s.replace(/^data:\s*/, ''))
  expect(events).toEqual(['tick 0', 'tick 1', 'tick 2'])
})

test('browser receives the events progressively (no full-body buffering)', async ({ page }) => {
  await page.goto('http://127.0.0.1:3105/')
  const chunkCount = await page.evaluate(async () => {
    const resp = await fetch('http://127.0.0.1:3105/api/events')
    if (!resp.body) throw new Error('no body')
    const reader = resp.body.getReader()
    const dec = new TextDecoder()
    let chunks = 0
    let sawAll = false
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      chunks += 1
      const text = dec.decode(value, { stream: true })
      if (text.includes('tick 2')) sawAll = true
    }
    // If the server buffered the whole body and sent it in one TCP write,
    // `chunks` is 1. The route emits 3 events spaced 30ms apart, so a
    // streaming pipeline shows multiple chunks. A single chunk does not
    // necessarily prove buffering (TCP can coalesce), but >= 2 is a strong
    // signal — the 30ms spacing across a real socket wins the race against
    // the transport.
    if (!sawAll) throw new Error('never saw tick 2')
    return chunks
  })
  expect(chunkCount).toBeGreaterThanOrEqual(2)
})
