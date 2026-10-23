// Contract: `exports` values point at existing files; `./defer` is
// advertised and resolves. Mirrors packages/svelte/test/export-map.test.ts
// — see that file's header for the bug class this guards against.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const pkgRoot = join(here, '..')
const pkg = JSON.parse(readFileSync(join(pkgRoot, 'package.json'), 'utf8'))

test('every exports value points at an existing file', () => {
  const missing: string[] = []
  for (const [subpath, target] of Object.entries(pkg.exports)) {
    const path = join(pkgRoot, target as string)
    if (!existsSync(path)) missing.push(`${subpath} -> ${target}`)
  }
  assert.deepEqual(missing, [], `exports point at missing files:\n  ${missing.join('\n  ')}`)
})

test('./defer is exported and resolves to a .ts file (the <Defer> component)', () => {
  assert.ok('./defer' in pkg.exports, `./defer must be in exports (got: ${Object.keys(pkg.exports).join(', ')})`)
  const target = pkg.exports['./defer'] as string
  assert.match(target, /defer\.ts$/, `./defer must point at defer.ts (got: ${target})`)
  assert.ok(existsSync(join(pkgRoot, target)), `./defer target does not exist: ${target}`)
})
