// install.js — the postinstall downloader's env-gated contract.
//
// The download itself needs network + a GitHub release, so the unit-level
// contract here is the skip/override behavior npm relies on:
//   - OXIDE_SKIP_DOWNLOAD=1   → print + skip, touch nothing
//   - OXIDE_BIN=<existing>    → print + skip (the user manages their binary)
// Both must be silent failures (a failed download must not break npm install).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, readdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const INSTALL = fileURLToPath(new URL('../install.js', import.meta.url))
const PKG_DIR = fileURLToPath(new URL('..', import.meta.url))

/** The postinstall cache lives INSIDE the package dir (`<pkg>/.oxide-bin`).
 *  A workspace checkout may already carry one (an earlier install ran the
 *  postinstall) — so the skip-path assertion is "no NEW files appear" and
 *  the tests clean up a dir they created themselves. */
function assertNoNewCacheArtifacts(createdByTest) {
  const cacheDir = join(PKG_DIR, '.oxide-bin')
  const files = existsSync(cacheDir) ? readdirSync(cacheDir) : []
  assert.deepEqual(files, [], 'skip paths must not download or write anything')
  if (createdByTest) rmSync(cacheDir, { recursive: true, force: true })
}

test('install: OXIDE_SKIP_DOWNLOAD=1 skips the download and touches nothing', () => {
  const cacheDir = join(PKG_DIR, '.oxide-bin')
  const existedBefore = existsSync(cacheDir)
  const r = spawnSync(process.execPath, [INSTALL], {
    cwd: PKG_DIR,
    env: { ...process.env, OXIDE_SKIP_DOWNLOAD: '1' },
    encoding: 'utf8',
  })
  assert.equal(r.status, 0)
  assert.match(r.stdout, /OXIDE_SKIP_DOWNLOAD=1, skipping binary download/)
  assertNoNewCacheArtifacts(!existedBefore && existsSync(cacheDir))
})

test('install: OXIDE_BIN pointing at an existing file wins over downloading', () => {
  const cacheDir = join(PKG_DIR, '.oxide-bin')
  const existedBefore = existsSync(cacheDir)
  const dir = mkdtempSync(join(tmpdir(), 'oxide-install-'))
  try {
    const fake = join(dir, 'my-oxide')
    writeFileSync(fake, '#!/bin/sh\nexit 0\n')
    const r = spawnSync(process.execPath, [INSTALL], {
      cwd: PKG_DIR,
      env: { ...process.env, OXIDE_BIN: fake },
      encoding: 'utf8',
    })
    assert.equal(r.status, 0)
    assert.match(r.stdout, new RegExp(`using OXIDE_BIN=${fake.replace(/[-/\\^$*+?.()|[\\]{}]/g, '\\$&')}`))
    assertNoNewCacheArtifacts(!existedBefore && existsSync(cacheDir))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
