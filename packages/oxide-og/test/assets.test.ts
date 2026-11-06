// assets.ts — brand PNG loading + font inlining.
//
// loadAssets() regressed once already (the require.main ESM bug): the default
// assets dir detection must work under BOTH tsx (src/) and tsc (dist/), and
// an explicit --input dir must always win.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { loadAssets, loadFonts, pngDataUri } from '../src/assets.ts'

const PKG_ASSETS = fileURLToPath(new URL('../assets/', import.meta.url))

test('og-assets: the package bundles all five brand PNGs', () => {
  for (const name of ['logo.png', 'react.png', 'vue.png', 'svelte.png', 'solid.png']) {
    assert.ok(pngDataUri(join(PKG_ASSETS, name)).startsWith('data:image/png;base64,'), `${name} loads`)
  }
})

test('og-assets: loadAssets with an explicit dir returns the five data URIs', () => {
  const assets = loadAssets(PKG_ASSETS)
  assert.deepEqual(Object.keys(assets).sort(), ['logo', 'react', 'solid', 'svelte', 'vue'])
  assert.ok(assets.logo.startsWith('data:image/png;base64,'))
  assert.ok(assets.react.length > 100, 'real PNG content, not an empty placeholder')
})

test('og-assets: loadAssets without a dir resolves the package default (ESM-safe)', () => {
  // Regression: the require.main-based default used to throw
  // "cannot determine module entrypoint" under ESM/tsx.
  const assets = loadAssets()
  assert.ok(assets.logo.startsWith('data:image/png;base64,'))
})

test('og-assets: a missing asset fails with the fix-it guidance naming the file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'og-assets-'))
  try {
    // One PNG present, one missing — the error must name the exact path.
    writeFileSync(join(dir, 'logo.png'), Buffer.from([0x89, 0x50, 0x4e, 0x47]))
    assert.throws(
      () => loadAssets(dir),
      (e: unknown) => {
        assert.ok(e instanceof Error)
        assert.match(e.message, /Missing required asset: .*react\.png/)
        assert.match(e.message, /--input <dir>/)
        return true
      },
    )
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('og-assets: loadFonts inlines the six @fontsource woff2 files', () => {
  const fonts = loadFonts()
  assert.deepEqual(Object.keys(fonts).sort(), ['inter500', 'inter600', 'inter700', 'inter800', 'mono400', 'mono500'])
  for (const [key, uri] of Object.entries(fonts)) {
    assert.ok(uri.startsWith('data:font/woff2;base64,'), `${key} is a woff2 data URI`)
    assert.ok(uri.length > 1000, `${key} carries real font bytes`)
  }
})
