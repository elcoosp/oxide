// cli.ts — the oxide-og command surface.
//
// These drive the REAL CLI entry via tsx (the same way `pnpm og` runs it):
// flag parsing defaults, validation errors, and — when a Chromium binary is
// available — the full render pipeline down to PNG bytes on disk.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, rmSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const PKG = fileURLToPath(new URL('..', import.meta.url))
const CLI = fileURLToPath(new URL('../src/cli.ts', import.meta.url))

function run(args: string[]) {
  return spawnSync(process.execPath, ['--import', 'tsx', CLI, ...args], {
    cwd: PKG,
    encoding: 'utf8',
    timeout: 180_000,
  })
}

test('og-cli: --help exits 0 and documents every flag', () => {
  const r = run(['--help'])
  assert.equal(r.status, 0)
  for (const flag of ['--input', '--out', '--sizes', '--theme', '--scale', '--headline', '--sub', '--repo', '--kicker', '--footer-left', '--footer-right']) {
    assert.ok(r.stdout.includes(flag), `help documents ${flag}`)
  }
})

test('og-cli: an unknown size fails with a clear validation error', () => {
  const r = run(['--sizes', 'og,banner'])
  assert.equal(r.status, 1)
  assert.match(r.stderr, /Unknown size "banner" \(expected og \| github \| all\)/)
})

test('og-cli: an unknown theme fails with a clear validation error', () => {
  const r = run(['--theme', 'sepia'])
  assert.equal(r.status, 1)
  assert.match(r.stderr, /Unknown theme "sepia" \(expected light \| dark\)/)
})

test('og-cli: missing assets fail with the fix-it guidance', () => {
  const dir = mkdtempSync(join(tmpdir(), 'og-cli-'))
  try {
    const r = run(['--input', dir])
    assert.equal(r.status, 1)
    assert.match(r.stderr, /Missing required asset: .*logo\.png/)
    assert.match(r.stderr, /--input <dir>/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('og-cli: full render — og + github sizes produce valid PNG files', { skip: !chromiumAvailable() }, () => {
  const out = mkdtempSync(join(tmpdir(), 'og-out-'))
  try {
    const r = run(['--input', join(PKG, 'assets'), '--out', out, '--sizes', 'all'])
    assert.equal(r.status, 0)
    assert.match(r.stdout, /og\.png/)
    assert.match(r.stdout, /og-github\.png/)
    // Default --scale 2: the PNG is supersampled to 2× for retina crispness.
    for (const [name, [w, h]] of [['og.png', [2400, 1260]], ['og-github.png', [2560, 1280]]] as const) {
      const p = join(out, name)
      assert.ok(existsSync(p), `${name} written`)
      const buf = readFileSync(p)
      assert.ok(buf.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])), `${name} is a PNG`)
      // PNG IHDR: width/height big-endian at fixed offsets.
      assert.equal(buf.readUInt32BE(16), w, `${name} width is ${w}px (scale 2)`)
      assert.equal(buf.readUInt32BE(20), h, `${name} height is ${h}px (scale 2)`)
    }
  } finally {
    rmSync(out, { recursive: true, force: true })
  }
})

test('og-cli: --scale 1 renders at the exact target pixels', { skip: !chromiumAvailable() }, () => {
  const out = mkdtempSync(join(tmpdir(), 'og-scale-'))
  try {
    const r = run(['--input', join(PKG, 'assets'), '--out', out, '--sizes', 'og', '--scale', '1'])
    assert.equal(r.status, 0)
    const buf = readFileSync(join(out, 'og.png'))
    assert.equal(buf.readUInt32BE(16), 1200)
    assert.equal(buf.readUInt32BE(20), 630)
  } finally {
    rmSync(out, { recursive: true, force: true })
  }
})

test('og-cli: dark theme renders a -dark suffixed file', { skip: !chromiumAvailable() }, () => {
  const out = mkdtempSync(join(tmpdir(), 'og-dark-'))
  try {
    const r = run(['--input', join(PKG, 'assets'), '--out', out, '--sizes', 'og', '--theme', 'dark'])
    assert.equal(r.status, 0)
    assert.ok(existsSync(join(out, 'og-dark.png')), 'og-dark.png written')
    assert.ok(!existsSync(join(out, 'og.png')), 'light og.png is NOT written in dark mode')
  } finally {
    rmSync(out, { recursive: true, force: true })
  }
})

/** Chromium availability probe — same cache playwright uses. When the
 *  browser is not installed the render tests SKIP instead of failing: the
 *  template/assets/CLI-flag contracts above still run everywhere. */
function chromiumAvailable(): boolean {
  try {
    const require = createRequire(import.meta.url)
    const { chromium } = require('playwright') as { chromium: { executablePath(): string | undefined } }
    const p = chromium.executablePath()
    return typeof p === 'string' && existsSync(p)
  } catch {
    return false
  }
}
