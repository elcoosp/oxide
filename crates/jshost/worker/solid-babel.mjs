// Pure-JS Solid JSX compiler fallback: babel-preset-solid.
//
// `solid-jsx-oxc` (the preferred Rust/OXC Solid compiler) ships per-platform
// NAPI binaries; when the binding for the running platform is absent
// (e.g. linux-x64, which upstream has not published yet), Solid `.tsx`
// compilation must still work — both for SSR (tsx-loader) and for the
// tooling-pool `solid-compile` op. `babel-preset-solid` produces the same
// runtime output shape (`ssr()` templates for SSR,
// `template()/insert()/createComponent()` for the DOM) and is a pure-JS
// dependency every Solid project already carries.
//
// Resolve `@babel/core` + `babel-preset-solid` from the PROJECT root first
// (they are the app's deps — e.g. examples/smoke-app-solid), then fall back
// to the worker's own resolution (a workspace-root install).

import { createRequire } from 'node:module'
import { join } from 'node:path'

const workerRequire = createRequire(import.meta.url)

const cacheByKey = new Map()

function loadFor(root) {
  const req = root ? createRequire(join(root, 'package.json')) : workerRequire
  let babel = null
  let preset = null
  try { babel = req('@babel/core') } catch { /* fall through to null */ }
  try { preset = req('babel-preset-solid') } catch { /* fall through to null */ }
  if (!babel || !preset) return null
  const presetFn = preset.default ?? preset
  if (typeof presetFn !== 'function') return null
  return { babel, presetFn }
}

/**
 * Load the babel Solid toolchain, preferring the given project root.
 * Returns `{ babel, presetFn }` or `null` when either package is missing.
 * Only POSITIVE results are cached per root — a miss (e.g. node_modules
 * pruned and re-created after this process started) must be retryable.
 */
export function loadSolidBabel(root) {
  const key = root || '<worker>'
  if (cacheByKey.get(key)) return cacheByKey.get(key)
  const loaded = loadFor(root)
  if (loaded) cacheByKey.set(key, loaded)
  return loaded
}

/**
 * Transform Solid JSX with babel-preset-solid.
 *
 * `opts` is passed straight to the preset — the two shapes this repo uses:
 *   - SSR:  `{ generate: 'ssr', hydratable: true, moduleName: '@solidjs/web' }`
 *   - DOM:  `{ generate: 'dom', moduleName: '@solidjs/web' }`
 *
 * `moduleName` is REQUIRED here: the preset's default is `solid-js/web`
 * (Solid 1.x); Solid 2.0 moved the runtime to `@solidjs/web` and the
 * runtime packages actually installed resolve there. `solid-jsx-oxc`'s
 * alpha had the same default and the same workaround (a specifier rewrite
 * after the fact) — passing the option is cleaner.
 *
 * After the preset runs, a compat pass renames delegated-event member keys
 * from the Solid-1 spelling (`$$click`, `$$clickData` — babel-preset-solid
 * 1.9 / dom-expressions 0.40 emit `$$<event>`) to the Solid 2 runtime's
 * spelling (`_$$click` — `EVENT_KEY = "_$$"` in @solidjs/web 2.x). Without
 * it the runtime's delegated handler looks up `_$$click`, finds nothing,
 * and every click is silently swallowed. The rename runs on the AST, so
 * user string literals containing `. $$` are never touched.
 */
export function solidBabelTransform(compiler, source, path, opts) {
  const { babel, presetFn } = compiler
  const t = babel.types
  const solid2EventKeyCompat = {
    name: 'oxide-solid2-event-key-compat',
    visitor: {
      MemberExpression(path) {
        const name = path.node.property?.name
        if (typeof name === 'string' && name.startsWith('$$')) {
          path.node.property = t.identifier(`_${name}`)
        }
      },
    },
  }
  const out = babel.transformSync(source, {
    filename: path,
    configFile: false,
    babelrc: false,
    presets: [[presetFn, opts]],
    plugins: [solid2EventKeyCompat],
    sourceMaps: false,
  })
  return out?.code ?? ''
}
