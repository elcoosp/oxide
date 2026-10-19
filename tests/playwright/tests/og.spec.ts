// SPEC-16.3 / §19.1: the `/_oxide/og` endpoint returns a real PNG.
//
// `SvgOgBackend` (via `resvg`+`usvg`+`tiny-skia`) is the rasterizer; the
// endpoint is wired into the server config in `start_server_inner_full`
// (`og_endpoint: build_og_endpoint()`). This test proves the whole chain —
// query parse → backend render → HTTP transport — works end-to-end.
//
// A full template engine for arbitrary OG metadata is a follow-up
// (the endpoint accepts `title` + `description` today).

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
  if (!existsSync(oxideBin)) throw new Error('oxide-cli missing')
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3106'], {
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch('http://127.0.0.1:3106/_oxide/health'); if (r.ok) return } catch {}
    await sleep(250)
  }
  throw new Error('oxide start did not come up')
})

test.afterAll(() => server?.kill('SIGTERM'))

test('GET /_oxide/og returns a PNG with the magic bytes', async ({ request }) => {
  const r = await request.get('http://127.0.0.1:3106/_oxide/og?title=Hello&description=World')
  expect(r.status()).toBe(200)
  expect(r.headers()['content-type']).toContain('image/png')
  const body = await r.body()
  // PNG magic: 0x89 'P' 'N' 'G' \r \n 0x1A \n
  expect(body.length).toBeGreaterThan(8)
  expect(body[0]).toBe(0x89)
  expect(body[1]).toBe(0x50) // 'P'
  expect(body[2]).toBe(0x4e) // 'N'
  expect(body[3]).toBe(0x47) // 'G'
  expect(body[4]).toBe(0x0d) // \r
  expect(body[5]).toBe(0x0a) // \n
  expect(body[6]).toBe(0x1a)
  expect(body[7]).toBe(0x0a)
})

test('GET /_oxide/og with no title returns 400', async ({ request }) => {
  const r = await request.get('http://127.0.0.1:3106/_oxide/og')
  expect(r.status()).toBe(400)
})

test('GET /_oxide/og twice with the same query returns identical bytes', async ({ request }) => {
  const url = 'http://127.0.0.1:3106/_oxide/og?title=Repeat&description=Same'
  const a = await request.get(url)
  const b = await request.get(url)
  expect(a.status()).toBe(200)
  expect(b.status()).toBe(200)
  const ba = await a.body()
  const bb = await b.body()
  expect(ba.equals(bb)).toBe(true)
})
