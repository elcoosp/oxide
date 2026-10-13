// Node ESM loader hook: transform .tsx / .jsx / .vue / .svelte to plain JS.
//
// - `.tsx` / `.jsx`: transformed with sucrase (JSX + TypeScript).
// - `.vue`:  parsed + compiled via `@vue/compiler-sfc` (inlineTemplate
//            so the emitted module has one default export).
// - `.svelte`: compiled via `svelte/compiler` with `generate: "ssr"`
//            so the module can be evaluated inside the SSR worker.
//
// `oxide dev` and `oxide render` load user code through this hook at
// module-evaluation time; `oxide build` uses the CLI precompile pass
// instead (see crates/cli/src/precompile.rs) which produces the same
// JS shape on disk.
//
// sucrase / @vue/compiler-sfc / svelte are resolved relative to this file,
// so the workspace root's node_modules must contain them (pnpm workspace
// install handles that). Absence of a given package degrades gracefully:
// the loader falls through to `nextLoad` and Node's native .ts stripping.
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)

function tryRequire(name) {
  try { return require(name) } catch { return null }
}

const sucrase = tryRequire('sucrase')
const vueSfc = tryRequire('@vue/compiler-sfc')
const svelteCompiler = tryRequire('svelte/compiler')

function compileTsx(source, path) {
  if (!sucrase) return null
  const result = sucrase.transform(source, {
    transforms: ['typescript', 'jsx'],
    jsxRuntime: 'automatic',
    production: true,
    filePath: path,
  })
  return result.code
}

function compileVue(source, path) {
  if (!vueSfc) return null
  const { descriptor, errors } = vueSfc.parse(source, { filename: path })
  if (errors && errors.length > 0) {
    throw new Error(`vue parse: ${errors[0].message ?? errors[0]}`)
  }
  // Short id — the compiler wants a stable per-file string for scoped ids.
  const shortId = Buffer.from(path).toString('base64').replace(/[^A-Za-z0-9]/g, '').slice(0, 8)
  const script = vueSfc.compileScript(descriptor, {
    id: shortId,
    inlineTemplate: true,
    templateOptions: { ssr: true },
  })
  return script.content
}

function compileSvelte(source, path) {
  if (!svelteCompiler) return null
  const compiled = svelteCompiler.compile(source, {
    filename: path,
    generate: 'ssr',
    css: 'injected',
  })
  return compiled?.js?.code ?? ''
}

export async function load(url, context, nextLoad) {
  const isTsx = url.endsWith('.tsx') || url.endsWith('.jsx')
  const isVue = url.endsWith('.vue')
  const isSvelte = url.endsWith('.svelte')
  if (!isTsx && !isVue && !isSvelte) {
    return nextLoad(url, context)
  }
  const path = fileURLToPath(url)
  const source = await readFile(path, 'utf8')
  let code
  if (isTsx) {
    code = compileTsx(source, path)
  } else if (isVue) {
    code = compileVue(source, path)
  } else {
    code = compileSvelte(source, path)
  }
  if (code == null) {
    // Compiler not installed; defer to the built-in loader.
    return nextLoad(url, context)
  }
  return {
    format: 'module',
    source: code,
    shortCircuit: true,
  }
}
