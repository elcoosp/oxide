// §1.4: CSS Modules end-to-end.
//
// Three things must hold simultaneously:
//
//  1. The client's `/@css/*.module.css` module exports a class map
//     (`{ hero: "_8Z4fiW_hero" }`), and a component using
//     `className={styles.hero}` gets that class on its DOM element.
//  2. SSR must produce the SAME mangled class on the server-rendered
//     element. Different hashes would break React hydration (#418 —
//     server markup disagrees with client). This is why the fix runs
//     the SAME Rust `LightningCssBackend` on both sides: `serve_css_module`
//     (client, on-demand) and `precompute_css_modules` (SSR, at boot,
//     read by the Node worker's `tsx-loader.mjs`).
//  3. HMR still works: editing the `.module.css` file updates both the
//     injected `<style>` (via the client module's re-import) AND the
//     precomputed SSR map (via the watcher's `.module.css` branch). A
//     full-page reload after the edit shows the new mangled name; an
//     in-place update changes the `<style>` text.

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3119

const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))
const CSS_MOD_PATH = join(smokeRoot, 'src', 'routes', 'index', 'hero.module.css')
const PAGE_MOD_PATH = join(smokeRoot, 'src', 'routes', 'index', 'page.tsx')

test.use({ baseURL: `http://127.0.0.1:${PORT}` })
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined
let cssPristine: string | null = null
let pagePristine: string | null = null

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) throw new Error(`oxide binary missing at ${oxideBin}`)
  if (existsSync(CSS_MOD_PATH)) cssPristine = readFileSync(CSS_MOD_PATH, 'utf8')
  if (existsSync(PAGE_MOD_PATH)) pagePristine = readFileSync(PAGE_MOD_PATH, 'utf8')

  const logTail: string[] = []
  const pushLog = (c: Buffer): void => {
    for (const line of c.toString('utf8').split('\n')) if (line) logTail.push(line)
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }
  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  server.stdout?.on('data', pushLog)
  server.stderr?.on('data', pushLog)

  let exited = false
  let info = ''
  server.once('exit', (code, signal) => { exited = true; info = `code=${code} signal=${signal}` })

  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (exited) throw new Error(`oxide dev exited (${info}). Last 50:\n${logTail.slice(-50).join('\n')}`)
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) { void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(10000) }).catch(() => {}); return }
    } catch { /* not up */ }
    await sleep(250)
  }
  try { server.kill('SIGKILL') } catch {}
  throw new Error(`oxide dev not serving within 30s. Last 50:\n${logTail.slice(-50).join('\n')}`)
})

test.afterAll(async () => {
  // Restore fixture files.
  try {
    if (cssPristine !== null) writeFileSync(CSS_MOD_PATH, cssPristine, 'utf8')
    else if (existsSync(CSS_MOD_PATH)) { /* created by test; unlink */ }
  } catch { /* nothing */ }
  try {
    if (pagePristine !== null) writeFileSync(PAGE_MOD_PATH, pagePristine, 'utf8')
  } catch { /* nothing */ }

  if (!server) return
  if (server.exitCode !== null || server.signalCode !== null) return
  const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()))
  server.kill('SIGTERM')
  const force = setTimeout(() => { try { server!.kill('SIGKILL') } catch {} }, 5000)
  await exited
  clearTimeout(force)
})

test.beforeEach(async () => {
  // Ensure the fixture is in its "installed" state for each test: the
  // module.css file exists and page.tsx imports + uses it. Both are
  // created fresh each test and restored by afterAll.
  if (pagePristine === null) return
  if (!pagePristine.includes("heroStyles from './hero.module.css'")) {
    // Add the import.
    const lines = pagePristine.split('\n')
    const last = lines.reduce((max, l, i) => (l.startsWith('import ') ? i : max), -1)
    lines.splice(last + 1, 0, "import heroStyles from './hero.module.css'")
    let withImport = lines.join('\n')
    withImport = withImport.replace(
      '<h1>Home</h1>',
      '<h1 data-cssmod={JSON.stringify(heroStyles)} className={heroStyles.hero}>Home</h1>',
    )
    writeFileSync(PAGE_MOD_PATH, withImport, 'utf8')
  }
  if (cssPristine === null || !existsSync(CSS_MOD_PATH)) {
    writeFileSync(
      CSS_MOD_PATH,
      '.hero {\n  color: rgb(200, 100, 50);\n}\n',
      'utf8',
    )
  }
  await sleep(300)
})

test('CSS module: client class, SSR class, and HMR update all agree (Tier 1.4)', async ({ page, request }) => {
  test.setTimeout(60_000)
  test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

  // [1] Client module: exports a class map with a mangled `hero`.
  const cssMod = await request.get('/@css/routes/index/hero.module.css')
  expect(cssMod.status()).toBe(200)
  const cssModBody = await cssMod.text()
  const exportMatch = cssModBody.match(/export default (\{"hero":"[^"]+"\})/)
  expect(exportMatch, `client /@css/ module missing class map: ${cssModBody.slice(0, 200)}`).not.toBeNull()
  const clientMap = JSON.parse(exportMatch![1]) as Record<string, string>
  expect(clientMap.hero, 'client map should have a mangled class').toMatch(/_\w+_hero$/)

  // [2] SSR HTML: <h1 class="<same-mangled>">.
  const html = await request.get('/')
  expect(html.status()).toBe(200)
  const htmlBody = await html.text()
  // Find the h1 with our Home text and check its class attr.
  const h1Match = htmlBody.match(/<h1[^>]*>Home<\/h1>/)
  expect(h1Match, 'Home h1 missing from SSR').not.toBeNull()
  const h1Tag = h1Match![0]
  const classAttr = h1Tag.match(/class="([^"]*)"/)?.[1] ?? ''
  expect(
    classAttr,
    `SSR h1 must carry the same mangled class as the client map. ` +
    `client=${clientMap.hero} ssr=${classAttr} | full h1: ${h1Tag.slice(0, 200)}`,
  ).toBe(clientMap.hero)

  // [3] Live page: after hydration, the client uses that same class.
  await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
  await page.waitForFunction(
    () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
    { timeout: 20_000 },
  )
  const domClass = await page.locator('h1', { hasText: 'Home' }).first().getAttribute('class')
  expect(domClass, 'client DOM h1 must carry the mangled class').toBe(clientMap.hero)

  // [4] HMR: edit the .module.css file — the injected <style> must update.
  // The class NAME stays the same (same rule name, same file → same hash);
  // only the CSS body changes. So the assertion is on style text, not on
  // the class string.
  const timeOrigin = await page.evaluate(() => performance.timeOrigin)
  const styleSelector = `style[data-oxide-css="routes/index/hero.module.css"]`
  await page.waitForFunction(
    (sel: string) => document.head?.querySelector(sel) !== null,
    styleSelector,
    { timeout: 15_000 },
  )

  writeFileSync(
    CSS_MOD_PATH,
    '.hero {\n  color: rgb(10, 200, 10);\n}\n',
    'utf8',
  )
  // lightningcss normalizes color syntax when it serializes: rgb(10, 200, 10)
  // becomes `#0ac80a`. That's what lands in the injected `<style>`. Assert the
  // normalized form — asserting the literal `rgb(...)` fails because the
  // serialized CSS never contains it. This is lightningcss's documented
  // behaviour; the exact hex is stable across versions because it's a
  // deterministic color serialization, not a hash.
  await expect
    .poll(
      () =>
        page.evaluate(
          (sel: string) => document.head?.querySelector(sel)?.textContent ?? '',
          styleSelector,
        ),
      { timeout: 15_000 },
    )
    .toContain('#0ac80a')

  expect(
    await page.evaluate(() => performance.timeOrigin),
    'CSS module HMR must not reload the page',
  ).toBe(timeOrigin)

  // [5] And the precomputed SSR map reflects the new file (via watcher's
  // `.module.css` branch → precompute_css_modules re-runs). The next SSR
  // request should see the fresh style baked in ... but the class NAME
  // doesn't change, so the only observable difference is the `<style>` in
  // the client. The SSR precompute is verified indirectly: if the watcher
  // didn't regenerate, the NEXT full page load would SSR-render with the
  // OLD CSS in `<style data-oxide-css>` (if SSR emitted style) — but SSR
  // doesn't inject style. So this aspect isn't end-to-end testable here;
  // it's covered by the `precompute_css_modules` unit test + the boot
  // precompute (which we test implicitly — the test only passes if SSR
  // map was precomputed correctly, in [2]).
})

// Ensure fixture restoration on SIGTERM too.
process.once('SIGTERM', () => {
  try {
    if (cssPristine !== null) writeFileSync(CSS_MOD_PATH, cssPristine, 'utf8')
    if (pagePristine !== null) writeFileSync(PAGE_MOD_PATH, pagePristine, 'utf8')
  } catch { /* nothing */ }
})
