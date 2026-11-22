// §V2-R01 C33 — experiment variants get separate L0 entries (F04).
//
// Two experiment buckets for one route get distinct L0 cache entries (the
// variant rides the cache key); a no-experiment route keeps the old key.
// Builds a temp fixture with an experiment on `/exp` (two buckets: A/B,
// each serving different content), starts `oxide start`, and asserts the two
// buckets serve distinct content (the variant split is in the cache key, so
// the buckets never collide). CI `playwright` job.
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
const base = 'http://127.0.0.1:3217'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c33-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(
    join(proj, 'oxide.config.json'),
    JSON.stringify({
      adapter: 'react',
      experiments: [{ route: '/exp', traffic: 100, variants: [{ name: 'A', weight: 50 }, { name: 'B', weight: 50 }] }],
    }),
  )
  writeFileSync(join(proj, 'src/routes/exp/page.tsx'), 'export default () => null')
  writeFileSync(
    join(proj, 'src/routes/exp/page.server.ts'),
    `export const config = { render: 'dynamic' }\n` +
      `export const load = async ({ locals }) => ({ variant: locals?.experiments?.bucket ?? 'none' })\n`,
  )
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3217'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('two experiment buckets serve distinct content (separate L0 entries)', async ({ request }) => {
  // Force the two buckets via the experiment cookie/header the runtime reads.
  // Two distinct buckets must serve distinct content — the variant rides the
  // cache key, so the buckets never collide in the L0 layer.
  const a = await request.get(`${base}/exp`, { headers: { 'x-oxide-experiment': 'A' } })
  const b = await request.get(`${base}/exp`, { headers: { 'x-oxide-experiment': 'B' } })
  expect(a.status()).toBe(200)
  expect(b.status()).toBe(200)
  const bodyA = await a.text()
  const bodyB = await b.text()
  // The two buckets carry their variant in the rendered output.
  expect(bodyA).not.toEqual(bodyB)
})
