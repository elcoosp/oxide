// Node ESM loader hook: transform .tsx / .jsx to plain JS via sucrase.
//
// Node's built-in type stripper handles .ts but cannot process JSX. This
// loader intercepts imports of .tsx / .jsx files, applies sucrase (JSX
// + TypeScript transforms), and returns the JS source so the worker can
// import a user's `page.tsx` directly.
//
// sucrase is resolved relative to THIS file, so the workspace root's
// node_modules must contain it (pnpm workspace install handles that).
//
// Registered by `NodePool::spawn` via the `--experimental-loader` flag.
// Files that don't end in .tsx/.jsx pass through unchanged by delegating
// to `nextLoad`, which is the built-in default loader (including Node's
// native type stripping for .ts).
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
let sucrase
try {
  sucrase = require('sucrase')
} catch (e) {
  // Lazy error: if the loader is registered but sucrase isn't installed,
  // surface a clear message on first .tsx import instead of at startup.
  sucrase = null
}

export async function load(url, context, nextLoad) {
  if (sucrase && (url.endsWith('.tsx') || url.endsWith('.jsx'))) {
    const path = fileURLToPath(url)
    const source = await readFile(path, 'utf8')
    const result = sucrase.transform(source, {
      transforms: ['typescript', 'jsx'],
      jsxRuntime: 'automatic',
      production: true,
      filePath: path,
    })
    return {
      format: 'module',
      source: result.code,
      shortCircuit: true,
    }
  }
  return nextLoad(url, context)
}
