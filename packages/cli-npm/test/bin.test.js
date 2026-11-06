// bin.js — the `oxide` bin shim: exec the cached binary (or OXIDE_BIN) with
// args/stdio/exit-code passthrough; exit 127 with the fix-it message when no
// binary is available. These tests drive the real shim as a child process —
// the same way npm execs it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const BIN = new URL('../bin.js', import.meta.url).pathname

/** A fake "oxide" binary: prints its args and exits with $FAKE_EXIT. */
function makeFakeBinary(dir) {
  const p = join(dir, 'fake-oxide')
  writeFileSync(p, '#!/bin/sh\necho "ARGS:$@"\necho "exit=$FAKE_EXIT"\nexit $FAKE_EXIT\n')
  chmodSync(p, 0o755)
  return p
}

test('bin: passes argv through to the binary and propagates its exit code', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oxide-bin-'))
  try {
    const fake = makeFakeBinary(dir)
    const r = spawnSync(process.execPath, [BIN, 'build', '--release'], {
      env: { ...process.env, OXIDE_BIN: fake, FAKE_EXIT: '7' },
      encoding: 'utf8',
    })
    assert.equal(r.status, 7, 'exit code must pass through')
    assert.match(r.stdout, /ARGS:build --release/, 'argv reaches the binary')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('bin: exit 0 passthrough (the happy path stays quiet)', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oxide-bin-'))
  try {
    const fake = makeFakeBinary(dir)
    const r = spawnSync(process.execPath, [BIN], {
      env: { ...process.env, OXIDE_BIN: fake, FAKE_EXIT: '0' },
      encoding: 'utf8',
    })
    assert.equal(r.status, 0)
    assert.equal(r.stderr, '')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('bin: a missing binary exits 127 with the fix-it guidance', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oxide-bin-'))
  try {
    const r = spawnSync(process.execPath, [BIN], {
      env: { ...process.env, OXIDE_BIN: join(dir, 'does-not-exist') },
      encoding: 'utf8',
    })
    assert.equal(r.status, 127)
    assert.match(r.stderr, /prebuilt CLI binary is not installed/)
    assert.match(r.stderr, /cargo install oxide-fw-cli/)
    assert.match(r.stderr, /OXIDE_BIN/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
