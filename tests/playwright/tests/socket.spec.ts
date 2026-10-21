// §9.6: WebSocket route E2E over `oxide start`.
//
// The smoke app's `routes/chat/socket.ts` maps to the WebSocket endpoint at
// `/chat`. Its handler:
//   * `open` sends `{ type: 'welcome' }` immediately;
//   * `message` echoes `{ type: 'echo', payload: <inbound> }`.
//
// This spec runs the WebSocket lifecycle inside the browser — the same
// transport a user's client code would use — and asserts the wire shape
// the Rust upgrade + framing produce end-to-end.

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
  server = spawn(oxideBin, ['start', '--root', smokeRoot, '--port', '3104'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch('http://127.0.0.1:3104/_oxide/health')
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

test('WebSocket at /chat greets on open and echoes on message', async ({ page }) => {
  await page.goto('http://127.0.0.1:3104/')
  await page.waitForFunction(() => typeof (window as any).WebSocket === 'function')

  const received = await page.evaluate(async () => {
    return await new Promise<string[]>((resolve, reject) => {
      const ws = new WebSocket('ws://127.0.0.1:3104/chat')
      const out: string[] = []
      const timeout = setTimeout(() => {
        ws.close()
        reject(new Error(`timed out; received ${JSON.stringify(out)}`))
      }, 10_000)
      ws.onopen = () => ws.send(JSON.stringify({ hello: 'from-test' }))
      ws.onmessage = (e) => {
        out.push(String(e.data))
        // Two messages: welcome + echo. Close once we have both.
        if (out.length === 2) {
          clearTimeout(timeout)
          ws.close()
          resolve(out)
        }
      }
      ws.onerror = (e) => {
        clearTimeout(timeout)
        reject(new Error(`ws error: ${String(e)}`))
      }
    })
  })

  expect(received).toHaveLength(2)
  const welcome = JSON.parse(received[0]) as { type: string }
  expect(welcome.type).toBe('welcome')
  const echo = JSON.parse(received[1]) as { type: string; payload: unknown }
  expect(echo.type).toBe('echo')
  // The server echoes the inbound text. The exact payload is whatever the
  // client sent (a JSON string), so just assert it survives the round-trip.
  expect(echo.payload).toBeDefined()
})

// ------------------------------ §9.6 / §22 / L6c: second client

/// Two concurrent clients on the same route must not interfere. This pins
/// the *isolation* of the per-connection lifecycle: a bug that assumed one
/// connection at a time (a shared `send` in the worker, a `OnceCell` for
/// the connection state, a Rust-side single-slot registry) would pass the
/// single-client test and fail here.
///
/// The H1 fan-out work (broadcast / rooms / targeting) is a separate WP —
/// this test deliberately does NOT assert that client B receives client
/// A's messages. The current `SocketService` has no connection registry;
/// `send` is per-connection only. When H1 lands, add a broadcast test
/// alongside this one; do not weaken this one to make room.
test('two clients — independent welcome + echo, no cross-talk', async ({ page }) => {
  await page.goto('http://127.0.0.1:3104/')
  await page.waitForFunction(() => typeof (window as any).WebSocket === 'function')

  const results = await page.evaluate(async () => {
    // Two connections opened in the same page (same origin as the single-
    // client test). Both should get exactly one welcome then one echo, and
    // neither should see the other's echo payload.
    const run = (marker: string) =>
      new Promise<{ welcome: any; echo: any; raw: string[] }>((resolve, reject) => {
        const ws = new WebSocket('ws://127.0.0.1:3104/chat')
        const out: string[] = []
        const timeout = setTimeout(() => {
          ws.close()
          reject(new Error(`client ${marker} timed out; got ${JSON.stringify(out)}`))
        }, 10_000)
        ws.onopen = () => ws.send(JSON.stringify({ who: marker }))
        ws.onmessage = (e) => {
          out.push(String(e.data))
          if (out.length === 2) {
            clearTimeout(timeout)
            ws.close()
            try {
              resolve({
                welcome: JSON.parse(out[0]),
                echo: JSON.parse(out[1]),
                raw: out,
              })
            } catch (err) {
              reject(err)
            }
          }
        }
        ws.onerror = (e) => {
          clearTimeout(timeout)
          reject(new Error(`client ${marker} ws error: ${String(e)}`))
        }
      })

    const [a, b] = await Promise.all([run('A'), run('B')])
    return { a, b }
  })

  // Each connection got its own welcome.
  expect(results.a.welcome.type).toBe('welcome')
  expect(results.b.welcome.type).toBe('welcome')
  // Each connection echoed its OWN payload — no cross-talk.
  expect(results.a.echo.type).toBe('echo')
  expect(results.b.echo.type).toBe('echo')
  // The echo payload is the client's own outbound JSON string.
  const aPayload = typeof results.a.echo.payload === 'string'
    ? results.a.echo.payload
    : JSON.stringify(results.a.echo.payload)
  const bPayload = typeof results.b.echo.payload === 'string'
    ? results.b.echo.payload
    : JSON.stringify(results.b.echo.payload)
  expect(aPayload).toContain('"who":"A"')
  expect(bPayload).toContain('"who":"B"')
  expect(aPayload).not.toContain('"who":"B"')
  expect(bPayload).not.toContain('"who":"A"')
})

