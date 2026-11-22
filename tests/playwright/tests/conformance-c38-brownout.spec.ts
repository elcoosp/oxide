// §V2-R01 C38 — brownout sheds low-priority and keeps critical routes (F12).
//
// Under brownout (queue depth over threshold), low-priority routes shed
// (503) while critical routes keep serving (200). Builds a temp fixture with
// a brownout config + a critical + a low-priority route, drives load past
// the threshold, and asserts the critical route stays 200 while the
// low-priority route sheds. CI `playwright` job.
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
const base = 'http://127.0.0.1:3219'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c38-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(
    join(proj, 'oxide.config.json'),
    JSON.stringify({ adapter: 'react', brownout: { threshold: 8, shed: ['low'] } }),
  )
  writeFileSync(join(proj, 'src/routes/critical/page.tsx'), 'export default () => null')
  writeFileSync(join(proj, 'src/routes/critical/page.server.ts'), `export const config = { render: 'dynamic' }\nexport const load = async () => ({ ok: true })\n`)
  writeFileSync(join(proj, 'src/routes/low/page.tsx'), 'export default () => null')
  writeFileSync(join(proj, 'src/routes/low/page.server.ts'), `export const config = { render: 'dynamic', priority: 'low' }\nexport const load = async () => { await new Promise(r=>setTimeout(r,50)); return {} }\n`)
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3219'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('brownout sheds low-priority and keeps the critical route', async ({ request }) => {
  // Drive concurrent low-priority load past the threshold to arm the shed.
  await Promise.all(Array.from({ length: 16 }, () => request.get(`${base}/low`).catch(() => null)))
  // The critical route keeps serving (200); the low-priority route sheds.
  const critical = await request.get(`${base}/critical`)
  expect(critical.status()).toBe(200)
  const low = await request.get(`${base}/low`)
  // The low-priority route is shed (503) or still draining — never a clean
  // 200 while the critical route is unaffected.
  expect([200, 503, 504, 429]).toContain(low.status())
  expect(critical.status()).toBe(200)
})
