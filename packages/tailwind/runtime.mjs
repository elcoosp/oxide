// @oxide-fw/tailwind/runtime.mjs
//
// Runtime bridge invoked by Oxide's tooling pool (crates/jshost). Runs in
// the V8 context — no subprocess. Two exported functions:
//
//   scan({sources, files})           → String[] (candidate class names)
//   build({css, base, candidates})   → { css: String }
//
// Design: docs/design-tailwind-v4.md §4.3.
//
// Only real Tailwind APIs are used:
//   - @tailwindcss/oxide :: Scanner  (Rust scanner, NAPI binding)
//   - tailwindcss       :: compile  (CSS compiler, JS)
//
// The codebase does not reimplement any Tailwind semantics. This file is
// just the glue between Oxide's tooling pool and Tailwind's two packages.

import { Scanner } from '@tailwindcss/oxide'
import { compile } from 'tailwindcss'
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'

// `createRequire(import.meta.url)` makes `require.resolve()` resolve
// relative to THIS module's location, not the caller's cwd. That's
// exactly what we need for Tailwind's own CSS files: they live in
// `node_modules/tailwindcss/`, wherever pnpm (or npm, or yarn) placed
// it. The caller's `base` is only used for the USER's own `@import`
// statements — Tailwind's own imports must resolve from the package.
const _require = createRequire(import.meta.url)

function _resolveTailwindCss(name) {
  try {
    return _require.resolve(name)
  } catch {
    return null
  }
}

const _TAILWIND_BUILTINS = {
  'tailwindcss': _resolveTailwindCss('tailwindcss/index.css'),
  'tailwindcss/index.css': _resolveTailwindCss('tailwindcss/index.css'),
  'tailwindcss/preflight': _resolveTailwindCss('tailwindcss/preflight.css'),
  'tailwindcss/preflight.css': _resolveTailwindCss('tailwindcss/preflight.css'),
  'tailwindcss/theme': _resolveTailwindCss('tailwindcss/theme.css'),
  'tailwindcss/theme.css': _resolveTailwindCss('tailwindcss/theme.css'),
  'tailwindcss/utilities': _resolveTailwindCss('tailwindcss/utilities.css'),
  'tailwindcss/utilities.css': _resolveTailwindCss('tailwindcss/utilities.css'),
}

async function _loadStylesheet(id, base) {
  // 1. Tailwind's own built-in CSS imports — resolve from THIS module,
  //    not from `base`.
  const builtin = _TAILWIND_BUILTINS[id]
  if (builtin) {
    const content = await fs.promises.readFile(builtin, 'utf-8')
    return { path: builtin, base: path.dirname(builtin), content }
  }

  // 2. Try normal resolution from the caller's base (user's `@import`
  //    statements).
  try {
    const resolved = _require.resolve(id, { paths: [base] })
    const content = await fs.promises.readFile(resolved, 'utf-8')
    return { path: resolved, base: path.dirname(resolved), content }
  } catch {}

  // 3. Try relative to base (for relative imports like `./theme.css`).
  const candidate = path.resolve(base, id.endsWith('.css') ? id : id + '.css')
  const content = await fs.promises.readFile(candidate, 'utf-8')
  return { path: candidate, base: path.dirname(candidate), content }
}

/**
 * Health check — the tooling pool calls this once at startup to fail fast
 * if `@oxide-fw/tailwind` isn't installed correctly.
 */
export async function ping() {
  return { ok: true, name: '@oxide-fw/tailwind', version: '0.1.0' }
}

/**
 * Scan sources for Tailwind class-name candidates.
 *
 * `sources` — array of `{base, pattern, negated}` glob sources (Tailwind's
 *             own `SourceEntry` shape). If `files` is provided, they take
 *             precedence and `sources` is only used as the scan root.
 * `files`   — array of `{file?, content?, extension}` changed content
 *             (Tailwind's `ChangedContent` shape). Used for incremental
 *             HMR scans and for scanning in-memory source strings without
 *             touching disk.
 *
 * Returns the deduplicated sorted candidate list.
 */
export async function scan({ sources, files }) {
  const scanner = new Scanner({ sources: sources ?? [] })
  if (Array.isArray(files) && files.length > 0) {
    return scanner.scanFiles(files)
  }
  return scanner.scan()
}

/**
 * Compile Tailwind CSS from an entry stylesheet plus a candidate set.
 *
 * `css`        — the entry stylesheet, typically `@import "tailwindcss";`
 *                plus optional `@theme {}` customizations.
 * `base`       — directory the entry lives in, for resolving user-land
 *                relative `@import`s. Tailwind's own package imports are
 *                resolved from `@oxide-fw/tailwind`'s install location.
 * `candidates` — class names collected by `scan()` (or by the CLI's graph).
 *
 * Returns `{ css: string }` — the full generated stylesheet. The CLI then
 * runs this through `LightningCssBackend` for browser targeting, prefixing,
 * and minification.
 */
export async function build({ css, base, candidates }) {
  const compiler = await compile(css, {
    base,
    loadStylesheet: async (id, importBase) => {
      const resolved = resolveStylesheet(id, importBase ?? base)
      return {
        path: resolved,
        base: path.dirname(resolved),
        content: fs.readFileSync(resolved, 'utf-8'),
      }
    },
  })
  return { css: compiler.build(candidates ?? []) }
}

/**
 * Resolve a stylesheet `@import` to an absolute filesystem path.
 *
 * Tailwind's own stylesheet imports (`@import "tailwindcss"`,
 * `@import "tailwindcss/preflight"`, etc.) resolve to the `tailwindcss`
 * package's own install location — which is the `@oxide-fw/tailwind` package's
 * `node_modules/tailwindcss/` directory. User-land imports (`@import "./x.css"`)
 * resolve relative to `base`.
 */
function resolveStylesheet(id, base) {
  if (id === 'tailwindcss' || id.startsWith('tailwindcss/')) {
    const relative =
      id === 'tailwindcss'
        ? 'index.css'
        : id.endsWith('.css')
          ? id.slice('tailwindcss/'.length)
          : `${id.slice('tailwindcss/'.length)}.css`
    const url = new URL(`./node_modules/tailwindcss/${relative}`, import.meta.url)
    return decodeFilePath(url)
  }
  return path.resolve(base, id)
}

/**
 * Convert a `file://` URL to a filesystem path. Uses `URL.pathname` directly
 * to avoid relying on `node:url`'s `fileURLToPath` — the tooling pool's V8
 * host provides `node:path` and `node:fs` (used by other tooling modules)
 * but `node:url` isn't part of the tested surface.
 */
function decodeFilePath(url) {
  let p = url.pathname
  try {
    p = decodeURIComponent(p)
  } catch {
    // leave encoded — exotic path; rare
  }
  // Windows: strip the leading `/` before `C:/...`
  if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1)
  return p
}
