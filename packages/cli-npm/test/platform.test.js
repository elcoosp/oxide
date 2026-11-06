// platform.js — the npm platform → release-asset mapping that BOTH the
// postinstall downloader and the bin shim depend on. The asset names must
// match .github/workflows/release-binaries.yml exactly:
//   oxide-<version>-<target>[.exe]
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { REPO, VERSION, PLATFORM_TARGETS, hostTarget, assetName, downloadBase } from '../platform.js'

test('platform: the target map covers all seven release targets', () => {
  // .github/workflows/release-binaries.yml builds exactly these (linux
  // gnu+musl × x64/arm64, macOS ×2, Windows x64).
  assert.deepEqual(Object.fromEntries(Object.entries(PLATFORM_TARGETS).sort()), {
    'linux-x64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
    'linux-musl-x64': 'x86_64-unknown-linux-musl',
    'linux-musl-arm64': 'aarch64-unknown-linux-musl',
    'darwin-x64': 'x86_64-apple-darwin',
    'darwin-arm64': 'aarch64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
  })
})

test('platform: VERSION matches this package and the repo slug is the release source', () => {
  const pkg = JSON.parse(readFileSync(fileURLToPath(new URL('../package.json', import.meta.url)), 'utf8'))
  assert.equal(VERSION, pkg.version)
  assert.equal(REPO, 'elcoosp/oxide')
})

test('platform: hostTarget maps the glibc/macos/windows combos', () => {
  assert.equal(hostTarget('linux', 'x64'), 'x86_64-unknown-linux-gnu')
  assert.equal(hostTarget('linux', 'arm64'), 'aarch64-unknown-linux-gnu')
  assert.equal(hostTarget('darwin', 'x64'), 'x86_64-apple-darwin')
  assert.equal(hostTarget('darwin', 'arm64'), 'aarch64-apple-darwin')
  assert.equal(hostTarget('win32', 'x64'), 'x86_64-pc-windows-msvc')
})

test('platform: unsupported combos resolve to null (install warns, bin exits 127)', () => {
  assert.equal(hostTarget('sunos', 'sparc'), null)
  assert.equal(hostTarget('freebsd', 'x64'), null)
  assert.equal(hostTarget('win32', 'arm64'), null)
  assert.equal(hostTarget('linux', 'riscv64'), null)
})

test('platform: assetName appends .exe only for windows targets', () => {
  assert.equal(assetName('x86_64-unknown-linux-gnu', '0.1.0'), 'oxide-0.1.0-x86_64-unknown-linux-gnu')
  assert.equal(assetName('aarch64-apple-darwin', '1.2.3'), 'oxide-1.2.3-aarch64-apple-darwin')
  assert.equal(assetName('x86_64-pc-windows-msvc', '0.1.0'), 'oxide-0.1.0-x86_64-pc-windows-msvc.exe')
})

test('platform: downloadBase defaults to the GitHub release and strips a trailing slash on overrides', () => {
  const original = process.env['OXIDE_CLI_DOWNLOAD_BASE']
  try {
    delete process.env['OXIDE_CLI_DOWNLOAD_BASE']
    assert.equal(downloadBase(), `https://github.com/elcoosp/oxide/releases/download/v${VERSION}`)
    process.env['OXIDE_CLI_DOWNLOAD_BASE'] = 'https://mirror.internal/oxide/'
    assert.equal(downloadBase(), 'https://mirror.internal/oxide', 'trailing slash must not double up')
    process.env['OXIDE_CLI_DOWNLOAD_BASE'] = 'https://mirror.internal/oxide/releases/v1'
    assert.equal(downloadBase(), 'https://mirror.internal/oxide/releases/v1')
  } finally {
    if (original === undefined) delete process.env['OXIDE_CLI_DOWNLOAD_BASE']
    else process.env['OXIDE_CLI_DOWNLOAD_BASE'] = original
  }
})
