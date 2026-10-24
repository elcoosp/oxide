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
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
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
  const [firstMsg, secondMsg] = received
  if (firstMsg === undefined || secondMsg === undefined) {
    throw new Error(`expected 2 messages, got ${received.length}`)
  }
  const welcome = JSON.parse(firstMsg) as { type: string }
  expect(welcome.type).toBe('welcome')
  const echo = JSON.parse(secondMsg) as { type: string; payload: unknown }
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
/// This test deliberately asserts isolation only. Fan-out is covered by the
/// dedicated broadcast test below, which uses the smoke handler's explicit
/// `{ type: 'broadcast' }` command so ordinary echoes stay isolated.
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
              const [first, second] = out
              if (first === undefined || second === undefined) {
                throw new Error(`expected 2 messages, got ${out.length}`)
              }
              resolve({
                welcome: JSON.parse(first),
                echo: JSON.parse(second),
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

// §9.6 / H1: `ctx.broadcast` reaches the other sockets on the same route,
// and `ctx.clients` exposes the live presence list.
test('broadcast fans out to peers and exposes presence', async ({ page }) => {
  await page.goto('http://127.0.0.1:3104/')
  await page.waitForFunction(() => typeof (window as any).WebSocket === 'function')

  const result = await page.evaluate(async () => {
    const openClient = () =>
      new Promise<{ ws: WebSocket; welcome: string }>((resolve, reject) => {
        const ws = new WebSocket('ws://127.0.0.1:3104/chat')
        const timer = setTimeout(() => reject(new Error('open timed out')), 10_000)
        ws.onmessage = (e) => {
          clearTimeout(timer)
          resolve({ ws, welcome: String(e.data) })
        }
        ws.onerror = (e) => { clearTimeout(timer); reject(new Error(String(e))) }
      })

    const a = await openClient()
    const b = await openClient()

    const peerFrame = new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('broadcast timed out')), 10_000)
      b.ws.onmessage = (e) => { clearTimeout(timer); resolve(String(e.data)) }
    })
    const presence = new Promise<any>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('presence timed out')), 10_000)
      a.ws.onmessage = (e) => {
        const msg = JSON.parse(String(e.data))
        if (msg.type === 'clients') { clearTimeout(timer); resolve(msg) }
      }
    })

    a.ws.send(JSON.stringify({ type: 'broadcast' }))
    const [frame, clients] = await Promise.all([peerFrame, presence])
    a.ws.close(); b.ws.close()
    return { frame: JSON.parse(frame), clients }
  })

  expect(result.frame.type).toBe('broadcast')
  expect(result.clients.type).toBe('clients')
  expect(Array.isArray(result.clients.clients)).toBe(true)
  expect(result.clients.clients.length).toBeGreaterThanOrEqual(2)
})

// §9.6: ctx.sendTo(id, data) delivers to one specific socket, not to the
// whole route. The last socket surface not covered above.
test('sendTo delivers to the target, not the sender', async ({ page }) => {
  await page.goto('http://127.0.0.1:3104/')
  await page.waitForFunction(() => typeof (window as any).WebSocket === 'function')

  const result = await page.evaluate(async () => {
    return await new Promise<{
      aId?: string
      bId?: string
      aDirect: any[]
      bDirect: any[]
    }>((resolve, reject) => {
      const a = new WebSocket('ws://127.0.0.1:3104/chat')
      const b = new WebSocket('ws://127.0.0.1:3104/chat')
      const s: { aId?: string; bId?: string; aDirect: any[]; bDirect: any[] } = {
        aDirect: [],
        bDirect: [],
      }
      const timeout = setTimeout(() => {
        a.close()
        b.close()
        reject(new Error('sendTo test timed out'))
      }, 5000)

      const fireIfReady = () => {
        if (s.aId && s.bId) {
          a.send(JSON.stringify({ type: 'sendto', to: s.bId, text: 'hello-B' }))
          setTimeout(() => {
            clearTimeout(timeout)
            a.close()
            b.close()
            resolve(s)
          }, 400)
        }
      }

      a.onmessage = (e) => {
        const m = JSON.parse(String(e.data))
        if (m.type === 'welcome') { s.aId = m.id; fireIfReady() }
        else if (m.type === 'direct') s.aDirect.push(m)
      }
      b.onmessage = (e) => {
        const m = JSON.parse(String(e.data))
        if (m.type === 'welcome') { s.bId = m.id; fireIfReady() }
        else if (m.type === 'direct') s.bDirect.push(m)
      }
    })
  })

  expect(result.bDirect.length, 'B received the targeted frame').toBe(1)
  expect(result.bDirect[0].type).toBe('direct')
  expect(result.bDirect[0].to).toBe(result.bId)
  expect(result.bDirect[0].text).toBe('hello-B')
  expect(result.aDirect.length, 'A (sender) did not receive').toBe(0)
})
