// Contract: every `exports` entry resolves to a real file. After the packaging
// change, exports values may be strings OR `{ types, default }` objects that
// point at compiled `dist/*.js` + `dist/*.d.ts` — not source `.ts` files.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const pkgRoot = join(here, '..')
const pkg = JSON.parse(readFileSync(join(pkgRoot, 'package.json'), 'utf8'))

/** Packaging: exports values may be strings OR {types, default} objects. */
function exportTarget(v: unknown): string | undefined {
  if (typeof v === 'string') return v
  if (v && typeof v === 'object') {
    const o = v as Record<string, unknown>
    if (typeof o.default === 'string') return o.default
    if (typeof o.types === 'string') return o.types
  }
  return undefined
}

test('every exports value points at an existing file', () => {
  const missing: string[] = []
  for (const [subpath, target] of Object.entries(pkg.exports)) {
    const resolved = exportTarget(target)
    if (!resolved) {
      missing.push(`${subpath} -> (unresolvable: ${JSON.stringify(target)})`)
      continue
    }
    if (!existsSync(join(pkgRoot, resolved))) missing.push(`${subpath} -> ${resolved}`)
  }
  assert.deepEqual(missing, [], `exports point at missing files:\n  ${missing.join('\n  ')}`)
})

test('./defer is exported and resolves to a file', () => {
  assert.ok('./defer' in pkg.exports, `./defer must be in exports (got: ${Object.keys(pkg.exports).join(', ')})`)
  const target = exportTarget(pkg.exports['./defer'])
  assert.ok(target, `./defer must have a resolvable target (got: ${JSON.stringify(pkg.exports['./defer'])})`)
  assert.match(target, /defer\.(ts|d\.ts|js)$/, `./defer must point at defer.{ts,js,d.ts} (got: ${target})`)
  assert.ok(existsSync(join(pkgRoot, target)), `./defer target does not exist: ${target}`)
})
