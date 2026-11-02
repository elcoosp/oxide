// §3.6: dev servers are isolated by design.
//
// The plan's original premise — "each process has its own broadcast; a WS
// to server A does not see edits on server B" — is CORRECT behaviour, not
// a bug. Two independent apps (e.g. `smoke-app-react` and
// `smoke-app-solid`) run separate dev servers on separate ports, watch
// separate source trees, and must NOT cross-deliver HMR messages. A
// cross-process broadcast would be a REGRESSION: editing a React fixture
// would force a reload in an unrelated Solid app on a different port.
//
// The other scenario the plan's "in-process broadcast" hint suggests —
// "two `oxide dev` instances watching the SAME project" — also resolves
// correctly today: both processes see the same filesystem events
// independently, so both publish their own ModuleUpdates to their own WS
// clients. Nothing is shared, nothing is missing.
//
// This test pins the isolation behaviour: editing a React fixture must
// reach the React dev server's WS subscribers and NOT the Solid one.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const reactRoot = join(repoRoot, 'examples', 'smoke-app-react')
const solidRoot = join(repoRoot, 'examples', 'smoke-app-solid')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const REACT_PORT = 3116
const SOLID_PORT = 3117

const HMR_AVAILABLE = existsSync(join(reactRoot, 'node_modules', 'react-refresh'))

test.describe.configure({ retries: 0 })
test.setTimeout(90_000)

let reactServer: ChildProcess | undefined
let solidServer: ChildProcess | undefined

async function spawnServer(rootDir: string, port: number): Promise<ChildProcess> {
  const s = spawn(oxideBin, ['dev', '--root', rootDir, '--port', String(port)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        void fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(10000) }).catch(() => {})
        return s
      }
    } catch { /* not up */ }
    await sleep(250)
  }
  try { s.kill('SIGKILL') } catch {}
  throw new Error(`server on ${port} did not start`)
}

async function killServer(s: ChildProcess | undefined): Promise<void> {
  if (!s) return
  if (s.exitCode !== null || s.signalCode !== null) return
  const exited = new Promise<void>((resolve) => s!.once('exit', () => resolve()))
  s.kill('SIGTERM')
  const force = setTimeout(() => { try { s!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
}

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  reactServer = await spawnServer(reactRoot, REACT_PORT)
  solidServer = await spawnServer(solidRoot, SOLID_PORT)
})

test.afterAll(async () => {
  await killServer(solidServer)
  await killServer(reactServer)
})

// React fixture (git-tracked? no — hardcode pristine bytes, same as §2.4).
const REACT_FIXTURE = join(reactRoot, 'src', 'components', 'hmr-target.tsx')
const REACT_PRISTINE = `import { useState } from 'react'\n\nexport default function HmrTarget() {\n  const [count, setCount] = useState(0)\n  return (\n    <>\n      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>\n      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>\n    </>\n  )\n}\n`

test.beforeEach(async () => {
  let cur = ''
  try { cur = readFileSync(REACT_FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== REACT_PRISTINE) {
    writeFileSync(REACT_FIXTURE, REACT_PRISTINE, 'utf8')
    await new Promise((r) => setTimeout(r, 300))
  }
})
test.afterEach(async () => {
  let cur = ''
  try { cur = readFileSync(REACT_FIXTURE, 'utf8') } catch { cur = '' }
  if (cur !== REACT_PRISTINE) writeFileSync(REACT_FIXTURE, REACT_PRISTINE, 'utf8')
  await new Promise((r) => setTimeout(r, 2000))
})

function captureFrames(page: import('@playwright/test').Page): string[] {
  const frames: string[] = []
  page.on('websocket', (ws) => {
    if (!ws.url().includes('/_oxide/hmr')) return
    ws.on('framereceived', (eventOrPayload: unknown) => {
      const raw = eventOrPayload && typeof eventOrPayload === 'object'
        && 'payload' in (eventOrPayload as object)
          ? (eventOrPayload as { payload: unknown }).payload
          : eventOrPayload
      let text: string
      if (typeof raw === 'string') text = raw
      else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw))
        text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
      else if (raw && typeof (raw as { toString?: unknown }).toString === 'function')
        text = (raw as { toString(enc: string): string }).toString('utf8')
      else text = String(raw)
      frames.push(text)
    })
  })
  return frames
}

test('editing a React fixture does NOT reach a Solid dev server (Tier 3.6)', async ({ browser }) => {
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed')

  const reactCtx = await browser.newContext()
  const solidCtx = await browser.newContext()
  const reactPage = await reactCtx.newPage()
  const solidPage = await solidCtx.newPage()

  const reactFrames = captureFrames(reactPage)
  const solidFrames = captureFrames(solidPage)

  try {
    // Boot both pages in parallel.
    await Promise.all([
      reactPage.goto(`http://127.0.0.1:${REACT_PORT}/`, { waitUntil: 'domcontentloaded', timeout: 30_000 }),
      solidPage.goto(`http://127.0.0.1:${SOLID_PORT}/`, { waitUntil: 'domcontentloaded', timeout: 30_000 }),
    ])
    await Promise.all([
      reactPage.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      ),
      solidPage.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      ),
    ])
    await sleep(800)

    const reactBefore = reactFrames.length
    const solidBefore = solidFrames.length

    // ONE edit — to the REACT fixture only.
    const marker = `ISOLATION-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`
    const patched = REACT_PRISTINE.replace(/HMR-TARGET-ORIGINAL/, marker)
    writeFileSync(REACT_FIXTURE, patched, 'utf8')

    // React's WS receives the ModuleUpdate (proves the edit propagated at all).
    await expect
      .poll(
        () => reactFrames.find((f) => f.includes('"type":"module-update"')) ?? null,
        { timeout: 15_000 },
      )
      .not.toBeNull()

    // Give the solid server a long window to receive any cross-process
    // leakage. It must receive ZERO new frames.
    await sleep(3000)

    const solidAfter = solidFrames.length
    const solidDelta = solidAfter - solidBefore
    expect(
      solidDelta,
      `§3.6 ISOLATION VIOLATED: the Solid dev server on port ${SOLID_PORT} received ` +
      `${solidDelta} new WS frames after an edit to a React fixture on port ${REACT_PORT}. ` +
      `Frames received by Solid: ${JSON.stringify(solidFrames.slice(solidBefore))}`,
    ).toBe(0)

    // And the React server received at least the one ModuleUpdate.
    expect(reactFrames.length).toBeGreaterThan(reactBefore)
  } finally {
    writeFileSync(REACT_FIXTURE, REACT_PRISTINE, 'utf8')
    await reactCtx.close()
    await solidCtx.close()
  }
})
