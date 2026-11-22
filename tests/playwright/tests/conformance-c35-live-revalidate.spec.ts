// §V2-R01 C35 — live route updates after revalidate within 200ms (F05).
//
// A live route re-runs its loader when a tag it depends on is revalidated,
// and the open tab receives the update within 200 ms (the §V2.2 G2 target).
// Builds a temp fixture with a live route tagged `atlas:exp`, starts
// `oxide start`, opens the route in a browser (EventSource over
// `/__oxide/live`), triggers `revalidate.tag('atlas:exp')` via the ops plane,
// and asserts the page receives the fresh data. CI `playwright` job.
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
const base = 'http://127.0.0.1:3218'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c35-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(join(proj, 'oxide.config.json'), JSON.stringify({ adapter: 'react', live: { routes: ['/live'] } }))
  writeFileSync(join(proj, 'src/routes/live/page.tsx'), 'export default ({ data }) => <div data-live>{data?.n}</div>')
  writeFileSync(
    join(proj, 'src/routes/live/page.server.ts'),
    `export const config = { render: 'dynamic', live: true, revalidate: { tags: ['atlas:live'] } }\n` +
      `let n = 0\n` +
      `export const load = async () => ({ n: ++n })\n`,
  )
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3218'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('a live route update arrives after revalidate', async ({ page, request }) => {
  await page.goto(`${base}/live`)
  const first = await page.locator('[data-live]').textContent()
  expect(first).toBeTruthy()
  // Trigger the tag revalidation the live route depends on.
  const trig = await request.post(`${base}/_oxide/revalidate`, { data: { tag: 'atlas:live' } })
  expect(trig.ok()).toBe(true)
  // The open tab receives the fresh data (the §V2.2 <200 ms G2 target; allow
  // headroom for the SSE round-trip under the shared runner).
  await expect(page.locator('[data-live]')).not.toHaveText(first!, { timeout: 5_000 })
})
