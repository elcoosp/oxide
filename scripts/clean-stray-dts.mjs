#!/usr/bin/env node
// Remove tsgo declaration emissions that leak into packages/*/src/.
//
// Trigger: a tsdown package's dts step (`generator: 'tsgo'`) compiles the
// whole program, including `@oxide-fw/core/*` imports that resolve to
// `packages/core/src/*.ts`. Because the *consumer's* tsconfig has
// `declaration: true`, tsgo emits sibling .d.ts/.d.ts.map next to each
// imported source file — i.e. into packages/core/src/.
//
// SAFETY: only delete X.d.ts / X.d.ts.map when X.ts exists as a sibling.
// That's the exact signature of a tsc/tsgo emission for a source file.
// Hand-written ambient declarations like packages/oxide-website/src/env.d.ts
// (no env.ts sibling) are preserved.

import { readdirSync, statSync, unlinkSync, existsSync } from 'node:fs'
import { join, relative, dirname, basename } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..')
const packagesDir = join(repoRoot, 'packages')
let removed = 0

function shouldDelete(file) {
  const name = basename(file)
  const dir = dirname(file)
  if (name.endsWith('.d.ts.map')) {
    // X.d.ts.map — delete only if X.ts exists
    const base = name.slice(0, -'.d.ts.map'.length)
    return existsSync(join(dir, base + '.ts'))
  }
  if (name.endsWith('.d.ts')) {
    // X.d.ts — delete only if X.ts exists (and X.d.ts is not the only
    // declaration for a directory index — index.ts sibling counts)
    const base = name.slice(0, -'.d.ts'.length)
    return existsSync(join(dir, base + '.ts'))
  }
  return false
}

function walk(dir) {
  let entries
  try { entries = readdirSync(dir) } catch { return }
  for (const name of entries) {
    const full = join(dir, name)
    let st
    try { st = statSync(full) } catch { continue }
    if (st.isDirectory()) walk(full)
    else if (shouldDelete(full)) {
      try { unlinkSync(full); removed++; console.log(`  removed ${relative(repoRoot, full)}`) }
      catch (e) { console.error(`  failed ${full}: ${e.message}`) }
    }
  }
}

for (const pkg of readdirSync(packagesDir)) {
  const src = join(packagesDir, pkg, 'src')
  try { if (!statSync(src).isDirectory()) continue } catch { continue }
  walk(src)
}
console.log(removed === 0 ? '  (no stray .d.ts with matching .ts sibling)' : `  removed ${removed} stray file(s)`)
