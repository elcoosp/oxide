// @oxide/oxide config helper tests (spec §14.1). `defineConfig` is an
// identity function at runtime; the tests pin that shape and the type
// export so a refactor can't accidentally remove it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { defineConfig } from '../src/index.ts'

test('defineConfig returns its argument unchanged', () => {
  const cfg = { adapter: 'binary', routes: { dir: 'src/routes' } }
  assert.equal(defineConfig(cfg), cfg, 'identity (same reference)')
})

test('defineConfig accepts a framework field of any shape', () => {
  const fakeFramework = { name: 'react' }
  const cfg = defineConfig({ framework: fakeFramework, adapter: 'binary' })
  assert.equal(cfg.framework, fakeFramework)
})

test('defineConfig is callable from a bare config module', async () => {
  // Write a tiny config file into a temp dir and import it — proves the
  // export is reachable from the same shape the CLI loads.
  //
  // The specifier inside the temp file must be ABSOLUTE (or a proper
  // relative path from the temp file), because the temp dir has no
  // node_modules to resolve `oxide` from. Anchoring on `import.meta.url`
  // keeps this robust to whatever the test runner's cwd is.
  const fs = await import('node:fs/promises')
  const os = await import('node:os')
  const path = await import('node:path')
  const url = await import('node:url')
  const here = path.dirname(url.fileURLToPath(import.meta.url))
  const indexPath = path.resolve(here, '..', 'src', 'index.ts')
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'oxide-defcfg-'))
  const file = path.join(dir, 'cfg.mjs')
  await fs.writeFile(file,
    `import { defineConfig } from '${url.pathToFileURL(indexPath).href}'\n` +
    `export default defineConfig({ adapter: 'binary' })\n`,
    'utf8')
  try {
    const mod = await import(url.pathToFileURL(file).href)
    assert.equal(mod.default.adapter, 'binary')
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})
