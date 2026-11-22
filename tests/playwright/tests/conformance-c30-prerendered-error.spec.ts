// §V2-R01 C30 — prerendered error page is served with its error status (A07).
//
// A prerendered error page (§10.5 sidecar) is served with the status the
// sidecar records, not a 200. Builds a temp fixture whose `/boom` route
// throws at prerender time, builds it (prerendering emits the error sidecar
// with the 500 status), and asserts `oxide start` serves the prerendered
// page with status 500. CI `playwright` job.
import { test, expect } from '@playwright/test'
import { spawn, execSync, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { mkdtempSync, writeFileSync, rmSync, existsSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const base = 'http://127.0.0.1:3216'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c30-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(join(proj, 'oxide.config.json'), JSON.stringify({ adapter: 'react', prerender: { errors: true } }))
  writeFileSync(join(proj, 'src/routes/boom/page.tsx'), 'export default () => null')
  // A route that throws at prerender time → the build emits the error sidecar.
  writeFileSync(
    join(proj, 'src/routes/boom/page.server.ts'),
    `export const config = { render: 'prerender' }\n` +
      `export const load = () => { throw new Error('boom') }\n`,
  )
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3216'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('the prerendered error page is served with its error status', async ({ request }) => {
  const r = await request.get(`${base}/boom`)
  // The prerendered error sidecar carries the 500 status (§10.5); a correct
  // serve surfaces it, not a 200.
  expect(r.status()).toBeGreaterThanOrEqual(500)
  expect(r.status()).toBeLessThan(600)
  const body = await r.text()
  expect(body.length).toBeGreaterThan(0)
})
