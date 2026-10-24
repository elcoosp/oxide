// WP-B (deviation 3): a route added mid-session must become reachable, and
// one deleted must 404 again, without restarting `oxide dev`.
//
// Modelled on loading.spec.ts: runs against the real smoke app (not a
// tmpdir copy), so pnpm's node_modules resolution is intact. The smoke app
// has a prebuilt `.oxide/routes.manifest` from a prior `oxide build`, which
// is exactly the archive-shadowing reproduction condition — dev must ignore
// it (`archived: None`) or the added route stays shadowed by the stale
// archive and still 404s.
//
// The test also asserts the pollution fix: after a full add/delete cycle,
// no `.<base>.<hash>.oxide.ts` or `*.oxide.css.mjs` may exist inside the
// smoke app's `src/`.

import { test, expect } from '@playwright/test'
import { spawn, execFileSync, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { existsSync, mkdirSync, rmSync, writeFileSync, unlinkSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const routeDir = join(smokeRoot, 'src', 'routes', 'rescan-me')
const routeFile = join(routeDir, 'page.tsx')
const PORT = 3105
const baseURL = `http://127.0.0.1:${PORT}`

let server: ChildProcess | undefined

test.beforeAll(async () => {
  test.setTimeout(90_000)
  if (!existsSync(oxideBin)) {
    throw new Error(
      `oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\` first`,
    )
  }
  // Clear any stale route file from a crashed prior run.
  try { rmSync(routeDir, { recursive: true, force: true }) } catch { /* ok */ }
  // Reproduction precondition: the prebuilt archive exists.
  if (!existsSync(join(smokeRoot, '.oxide', 'routes.manifest'))) {
    throw new Error(
      `expected ${smokeRoot}/.oxide/routes.manifest; the smoke app must ` +
        `have a prebuilt archive for this test to reproduce the ` +
        `archive-shadowing bug`,
    )
  }

  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  })
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`${baseURL}/_oxide/health`)
      if (r.ok) return
    } catch { /* not up */ }
    await sleep(250)
  }
  throw new Error('oxide dev did not come up within 60s')
})

test.afterAll(async () => {
  try { server?.kill('SIGTERM') } catch { /* ok */ }
  await sleep(500)
  try { server?.kill('SIGKILL') } catch { /* ok */ }
  try { rmSync(routeDir, { recursive: true, force: true }) } catch { /* ok */ }
})

test('a route added mid-session is reachable without restarting dev', async () => {
  // Baseline: new route 404s.
  const before = await fetch(`${baseURL}/rescan-me`)
  expect(before.status).toBe(404)

  // Baseline: home renders. Two requests — the second would 503 under the
  // pre-fix watcher self-trigger bug. Capture the body on failure so a
  // 500 (e.g. a `Cannot find package` diagnostic) is legible.
  const home1 = await fetch(`${baseURL}/`)
  if (home1.status !== 200) {
    const body = await home1.text()
    throw new Error(
      `first GET / returned ${home1.status}; body: ${body.slice(0, 500)}`,
    )
  }
  const home2 = await fetch(`${baseURL}/`)
  if (home2.status !== 200) {
    const body = await home2.text()
    throw new Error(
      `second GET / returned ${home2.status} (should be 200; 503 = ` +
        `stale-generation self-trigger); body: ${body.slice(0, 500)}`,
    )
  }

  // Add the route.
  mkdirSync(routeDir, { recursive: true })
  writeFileSync(
    routeFile,
    `export default function Page() {
  return <section data-rescan-page><h1>rescan</h1></section>
}
`,
    'utf8',
  )

  // Poll for the dev watcher rescan.
  let sawNew = false
  let lastStatus = 0
  const addDeadline = Date.now() + 15_000
  while (Date.now() < addDeadline) {
    const r = await fetch(`${baseURL}/rescan-me`)
    lastStatus = r.status
    if (r.status === 200) {
      sawNew = true
      expect(await r.text()).toContain('rescan')
      break
    }
    await sleep(200)
  }
  expect(sawNew, `new route reachable (last status ${lastStatus})`).toBe(true)

  // Delete the route.
  unlinkSync(routeFile)

  // Poll for the dev watcher to remove it.
  let sawGone = false
  let goneStatus = 0
  const delDeadline = Date.now() + 15_000
  while (Date.now() < delDeadline) {
    const r = await fetch(`${baseURL}/rescan-me`)
    goneStatus = r.status
    if (r.status === 404) { sawGone = true; break }
    await sleep(200)
  }
  expect(sawGone, `deleted route 404s again (last status ${goneStatus})`).toBe(true)

  // Pollution check: no compiled artifacts inside src/.
  let pollution = ''
  try {
    pollution = execFileSync(
      'bash',
      ['-c', `find '${smokeRoot}/src' \\( -name '.*.oxide.ts' -o -name '*.oxide.css.mjs' \\)`],
    ).toString().trim()
  } catch { /* find exits non-zero when nothing matches; treat as clean */ }
  expect(pollution, 'src/ must not contain compiled artifacts').toBe('')
})
