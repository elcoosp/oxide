// Contract: the subpaths the package advertises in `exports` actually
// resolve to files that exist, and the ones whose feature is "shipped"
// are not silently missing from the map. This catches the class of bug
// where a component ships but is never reachable to a consumer —
// exactly what happened to Vue's `defer.ts` (existed on disk, absent
// from `exports`) and to Svelte's `Defer.svelte` (added, then the
// export was forgotten).
//
// If a subpath entry's `exports` value points at a file that doesn't
// exist, or a required subpath is missing, this test fails. Adding a
// new subpath is: add the file, add the export entry — this test keeps
// you honest about doing both.
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

test('./defer is exported and resolves to a .svelte file (the <Defer> component)', () => {
  assert.ok('./defer' in pkg.exports, `./defer must be in exports (got: ${Object.keys(pkg.exports).join(', ')})`)
  const target = pkg.exports['./defer'] as string
  assert.match(target, /Defer\.svelte$/, `./defer must point at Defer.svelte (got: ${target})`)
  assert.ok(existsSync(join(pkgRoot, target)), `./defer target does not exist: ${target}`)
})
