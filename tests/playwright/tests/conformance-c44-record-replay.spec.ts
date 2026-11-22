// §V2-R01 C44 — record then replay is byte-identical (F15).
//
// A recorded request/response pair replays byte-identical (the
// interaction-replay contract). Builds a temp fixture, records a request
// under `OXIDE_RECORD=1`, then re-runs the same request under
// `OXIDE_REPLAY=1` and asserts the replayed response is byte-identical to
// the recorded one (the record carries no unredacted secrets by default).
// CI `playwright` job.
import { test, expect } from '@playwright/test'
import { spawn, execSync, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { mkdtempSync, writeFileSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const base = 'http://127.0.0.1:3221'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c44-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(join(proj, 'oxide.config.json'), JSON.stringify({ adapter: 'react' }))
  writeFileSync(join(proj, 'src/routes/index/page.tsx'), 'export default () => null')
  writeFileSync(join(proj, 'src/routes/index/page.server.ts'), `export const config = { render: 'dynamic' }\nexport const load = async () => ({ msg: 'replay-me' })\n`)
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  // Record pass: capture the response under OXIDE_RECORD.
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3221'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, OXIDE_RECORD: '1' },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('record then replay is byte-identical', async ({ request }) => {
  // Record pass: the response is captured to .oxide/records.
  const recorded = await request.get(`${base}/__data?route=/`)
  expect(recorded.status()).toBe(200)
  const recordedBody = await recorded.text()
  expect(recordedBody).toContain('replay-me')
  // Replay pass: the same request under OXIDE_REPLAY yields byte-identical
  // bytes (the record is the oracle; the live loader is never consulted).
  server?.kill('SIGTERM')
  await sleep(500)
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3221'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, OXIDE_REPLAY: '1' },
  })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) break } catch { /* not up */ }
    await sleep(250)
  }
  const replayed = await request.get(`${base}/__data?route=/`)
  expect(replayed.status()).toBe(200)
  const replayedBody = await replayed.text()
  expect(replayedBody).toBe(recordedBody)
})
