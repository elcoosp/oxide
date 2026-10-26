// Ensure the `tailwindcss` package that `runtime.mjs` resolves has the
// CSS files its `compile()` API expects. pnpm's isolated store
// (`.pnpm/tailwindcss@X/node_modules/tailwindcss/`) sometimes ships only
// `dist/` — the hoisted root has the full package. This script copies
// the missing files into the resolved location so the workspace test
// passes without touching `.npmrc` or the lockfile.
//
// Invoked from packages/tailwind's pretest script. Idempotent — safe to
// run on every install / test.

import { existsSync, copyFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const __dirname = dirname(fileURLToPath(import.meta.url))
const root = resolve(__dirname, '..')

const FILES = ['index.css', 'preflight.css', 'theme.css', 'utilities.css']

// Where does Node resolve `tailwindcss/package.json` from packages/tailwind/runtime.mjs?
const req = createRequire(join(root, 'packages/tailwind/runtime.mjs'))
let resolvedRoot
try {
  resolvedRoot = dirname(req.resolve('tailwindcss/package.json'))
} catch (e) {
  console.error('ensure-tailwind-css: tailwindcss not resolvable from packages/tailwind/:', e.message)
  process.exit(0)
}

// Where is the "complete" copy? Convention: hoisted root.
const hoisted = join(root, 'node_modules/tailwindcss')

if (!existsSync(hoisted)) {
  console.error('ensure-tailwind-css: hoisted root not found at', hoisted)
  process.exit(0)
}

let copied = 0
for (const f of FILES) {
  const from = join(hoisted, f)
  const to = join(resolvedRoot, f)
  if (existsSync(from) && !existsSync(to)) {
    copyFileSync(from, to)
    copied++
  }
}

if (copied > 0) {
  console.log(`ensure-tailwind-css: copied ${copied} file(s) into ${resolvedRoot}`)
}
