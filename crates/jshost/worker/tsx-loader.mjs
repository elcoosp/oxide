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
import { readFileSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { join } from 'node:path'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)

function tryRequire(name) {
  try { return require(name) } catch { return null }
}

const sucrase = tryRequire('sucrase')
const vueSfc = tryRequire('@vue/compiler-sfc')
const svelteCompiler = tryRequire('svelte/compiler')
const solidJsxOxc = (() => {
  const m = tryRequire('solid-jsx-oxc')
  return m ? (m.default ?? m) : null
})()

// §6.3: which framework's JSX compiler to use for `.tsx`/`.jsx`. The CLI
// sets this before spawning the worker (the adapter name from the loaded
// config): `react` (default) or `solid`. Vue and Svelte entries have their
// own SFC compilers below.
const FRAMEWORK = (process.env.OXIDE_FRAMEWORK ?? 'react').toLowerCase()

// §13.3 / SPEC-14: HMR module-graph versioning.
//
// The config file imports page/layout components STATICALLY:
//   import Home from './src/routes/index/page.tsx'
// When dev re-imports the config at `config.ts?v=<gen>`, that produces a
// fresh config module — but its STATIC imports still resolve to the SAME
// URL as before, so Node's ESM cache serves the old module.
//
// Fix: in dev, version every component file under <OXIDE_ROOT>/src/ with
// a `?v=<gen>` suffix, mirroring what the loader path already does for
// page.server.ts via content-hashed temp files. The generation is written
// by the Rust watcher on each `framework:invalidate` (see
// `crates/cli/src/lib.rs::invalidate_framework_modules`), so every file
// edit produces a fresh URL for the transitively-imported components.
const OXIDE_DEV = process.env.OXIDE_DEV === '1'
const OXIDE_ROOT = process.env.OXIDE_ROOT || ''

function currentGeneration() {
  if (!OXIDE_ROOT) return '0'
  try {
    return readFileSync(join(OXIDE_ROOT, '.oxide/hmr-generation'), 'utf8').trim() || '0'
  } catch {
    return '0'
  }
}

// A component file the loader can meaningfully re-evaluate. If it's not
// one of these, or it already has a `?v=`, leave the URL alone.
function isDevComponentUrl(url) {
  if (!url.startsWith('file://')) return false
  if (url.includes('node_modules')) return false
  if (url.includes('?v=')) return false
  if (!/\.(tsx|jsx|vue|svelte)(\?|$)/.test(url)) return false
  if (!OXIDE_ROOT) return false
  // Must be under <root>/src/
  const srcPrefix = 'file://' + OXIDE_ROOT.replace(/\/+$/, '') + '/src/'
  return url.startsWith(srcPrefix)
}

export async function resolve(specifier, context, nextResolve) {
  const result = await nextResolve(specifier, context)
  if (!OXIDE_DEV) return result
  if (!isDevComponentUrl(result.url)) return result
  const gen = currentGeneration()
  const sep = result.url.includes('?') ? '&' : '?'
  return { ...result, url: result.url + sep + 'v=' + gen }
}

// SPEC-15-C: dev-time plugin transform. `oxide dev` writes the list of
// `kind: "js"` plugins to `.oxide/dev-plugins.json` at start. Each is a
// Node ESM module; we import it ONCE (cache by path) and call its
// `transform` hook on every `.ts/.tsx/.js/.jsx` file under `<root>/src/`.
//
// This closes the dev-vs-build gap: without it, `oxide dev` compiles
// user code with sucrase only, while `oxide build` runs the plugin chain
// via the bundler's `transform_hook` — two paths that can diverge.
//
// Signature parity: the worker's `dispatchPluginCall` calls
// `fn.call(ctx, args)` where `args = { code, id }`. We do the same here
// so a plugin sees the same shape in dev and build.
let __devPluginCache = null; // { plugins: [{path, mod}] } | null
let __devPluginLoading = false;

async function loadDevPlugins() {
  if (__devPluginCache !== null) return __devPluginCache;
  if (__devPluginLoading) return null; // re-entrancy guard: a plugin transform importing another module
  if (!OXIDE_DEV || !OXIDE_ROOT) {
    __devPluginCache = { plugins: [] };
    return __devPluginCache;
  }
  __devPluginLoading = true;
  try {
    const registryPath = join(OXIDE_ROOT, '.oxide/dev-plugins.json');
    let entries = [];
    try {
      entries = JSON.parse(readFileSync(registryPath, 'utf8'));
    } catch {
      __devPluginCache = { plugins: [] };
      return __devPluginCache;
    }
    if (!Array.isArray(entries)) entries = [];
    const plugins = [];
    for (const entry of entries) {
      const path = entry && entry.path;
      if (typeof path !== 'string') continue;
      try {
        const mod = await import(pathToFileURL(path).href);
        const plugin = mod.default ?? mod;
        if (plugin && typeof plugin.transform === 'function') {
          plugins.push({ path, mod: plugin });
        }
      } catch (e) {
        process.stderr.write(
          '[oxide] plugin load failed for ' + path + ': ' + (e?.message ?? e) + '\n',
        );
      }
    }
    __devPluginCache = { plugins };
    return __devPluginCache;
  } finally {
    __devPluginLoading = false;
  }
}

async function runPluginTransforms(code, id) {
  const cache = await loadDevPlugins();
  if (!cache || cache.plugins.length === 0) return code;
  let out = code;
  for (const { path, mod } of cache.plugins) {
    try {
      const ctx = {
        warn: (m) => process.stderr.write('[plugin ' + path + '] ' + String(m) + '\n'),
        error: (m) => { throw new Error(String(m)); },
        resolve: async (spec) => spec,
      };
      // Hooks are positional (`transform(code, id)`), matching the build-time
      // worker. The old object-argument call passed `{code, id}` as `code`,
      // so a plugin's `code.replace(...)` threw or returned nothing — the
      // exact bug the build worker had.
      const result = await mod.transform.call(ctx, out, id);
      if (typeof result === 'string') {
        out = result;
      } else if (result && typeof result.code === 'string') {
        out = result.code;
      }
    } catch (e) {
      process.stderr.write(
        '[oxide] plugin transform failed for ' + path + ' on ' + id + ': ' +
          (e?.message ?? e) + '\n',
      );
    }
  }
  return out;
}

function compileTsx(source, path) {
  // Solid's JSX compiles to `ssr`/template calls, not React's `jsx()`
  // factories — `solid-jsx-oxc` (Rust/OXC, NAPI) is the compiler for it.
  // Its output keeps TS annotations, so a follow-up type-strip runs after.
  if (FRAMEWORK === 'solid' && solidJsxOxc) {
    const out = solidJsxOxc.transform(source, {
      moduleName: '@solidjs/web',
      generate: 'ssr',
      hydratable: true,
      filename: path,
    })
    let js = typeof out === 'string' ? out : out.code
    // The alpha ignores `moduleName` and emits `solid-js/web`; Solid 2.0
    // moved the runtime to `@solidjs/web`. Rewrite the specifier.
    js = js.replaceAll('from "solid-js/web"', 'from "@solidjs/web"')
           .replaceAll("from 'solid-js/web'", "from '@solidjs/web'")
    if (sucrase) {
      js = sucrase.transform(js, {
        transforms: ['typescript'],
        production: true,
        filePath: path,
      }).code
    }
    return js
  }
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
  // `compileScript` preserves TypeScript annotations (`lang="ts"` in the
  // `<script setup>` block). Node only strips types from sources it loads
  // off disk with a `.ts`/`.mts`/`.cts` extension — a `.vue` loader hook
  // must strip them itself, or the runtime hits "Unexpected token ':'".
  const ts = script.content
  if (sucrase) {
    return sucrase.transform(ts, {
      transforms: ['typescript'],
      production: true,
      filePath: path,
    }).code
  }
  return ts
}

function compileSvelte(source, path) {
  if (!svelteCompiler) return null
  const compiled = svelteCompiler.compile(source, {
    filename: path,
    generate: 'ssr',
    css: 'injected',
  })
  const js = compiled?.js?.code ?? ''
  // Svelte 5 runs `<script lang="ts">` through its own type stripper when
  // it can find one, but without a preprocessor the types pass through.
  // Strip defensively — sucrase leaves plain JS untouched.
  if (sucrase) {
    try {
      return sucrase.transform(js, {
        transforms: ['typescript'],
        production: true,
        filePath: path,
      }).code
    } catch {
      return js
    }
  }
  return js
}

export async function load(url, context, nextLoad) {
  // SPEC-14: the resolve() hook appends `?v=<gen>` to component URLs for
  // HMR cache-busting. The extension checks below must use the BARE path —
  // `url.endsWith('.tsx')` on `page.tsx?v=35` is false.
  const queryAt = url.indexOf('?')
  const bareUrl = queryAt >= 0 ? url.slice(0, queryAt) : url
  // BUG-08: CSS is a server side-effect import — evaluate to an empty
  // module (SSR never applies styles; the client bundle handles real CSS).
  if (bareUrl.endsWith('.css')) {
    return { format: 'module', shortCircuit: true, source: 'export default "";' }
  }
  const isTsx = bareUrl.endsWith('.tsx') || bareUrl.endsWith('.jsx')
  const isVue = bareUrl.endsWith('.vue')
  const isSvelte = bareUrl.endsWith('.svelte')
  if (!isTsx && !isVue && !isSvelte) {
    return nextLoad(url, context)
  }
  const path = fileURLToPath(bareUrl)
  let source = await readFile(path, 'utf8')
  // SPEC-15-C: run the dev-time plugin transform on the RAW source (before
  // the framework compile) so a plugin sees the source a user authored —
  // same order as the bundler (`TransformHook` runs before Oxc passes).
  // Restricted to `<root>/src/` so a plugin never transforms framework
  // internals or node_modules.
  if (OXIDE_DEV && OXIDE_ROOT) {
    const srcPrefix = OXIDE_ROOT.replace(/\/+$/, '') + '/src/'
    if (path.startsWith(srcPrefix)) {
      source = await runPluginTransforms(source, path)
    }
  }
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
