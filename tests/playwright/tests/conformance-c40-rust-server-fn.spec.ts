// §V2-R01 C40 — Rust server function callable from the client (F08, binary only).
//
// A `*.server.rs` export is callable from the client over `/_oxide/fn/<id>`
// (the Rust function host, Binary/Docker only — the matrix refuses `rustFns`
// elsewhere). Builds a temp fixture with a `*.server.rs` function, builds
// with `rustFns: true`, starts `oxide start` (the binary server), and asserts
// the client can call the Rust fn over the ops plane. CI `playwright` job.
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
const base = 'http://127.0.0.1:3220'
const proj = mkdtempSync(join(tmpdir(), 'oxide-c40-'))

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(120_000)
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`)
  writeFileSync(join(proj, 'oxide.config.json'), JSON.stringify({ adapter: 'react', rustFns: true }))
  writeFileSync(join(proj, 'src/routes/index/page.tsx'), 'export default () => null')
  // A Rust server function: the host loads it as a cdylib and exports it over
  // `/_oxide/fn/<id>`. The ABI is bytes-in / bytes-out (§V2-F08).
  writeFileSync(
    join(proj, 'src/routes/index/rust.server.rs'),
    `#[oxide_fw::server_fn(name = "rust-hello")]\n` +
      `pub fn rust_hello(who: &str) -> String { format!("hello from rust, {}", who) }\n`,
  )
  execSync(`"${oxideBin}" build --root "${proj}"`, { stdio: 'inherit' })
  server = spawn(oxideBin, ['start', '--root', proj, '--port', '3220'], { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env } })
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { const r = await fetch(`${base}/_oxide/health`); if (r.ok) return } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide start did not come up within 30s')
})

test.afterAll(() => { server?.kill('SIGTERM'); rmSync(proj, { recursive: true, force: true }) })

test('a Rust server function is callable from the client', async ({ request }) => {
  const r = await request.post(`${base}/_oxide/fn/rust-hello`, {
    headers: { 'content-type': 'application/json' },
    data: { args: ['Ada'] },
  })
  expect(r.status()).toBe(200)
  const body = await r.json()
  expect(JSON.stringify(body)).toContain('hello from rust')
})
