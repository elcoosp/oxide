// @oxide/vue real Vue SSR path (spec §7).
//
// The IR-based renderer in ./render.ts is the NEUTRAL fallback: it runs with
// no framework deps installed and drives the §22 conformance suite. This
// module is the REAL path: it dynamically imports `vue` and
// `@vue/server-renderer` and renders an actual Vue component tree through
// the real upstream renderer.
//
// Imports are dynamic and cached so the package keeps working (falling back
// to the IR renderer) when vue/@vue/server-renderer are absent.

export interface VueAppLike {
  createSSRApp(component: unknown, props?: Record<string, unknown>): unknown
  h(type: unknown, props?: Record<string, unknown> | null, ...children: unknown[]): unknown
  defineComponent(opts: Record<string, unknown>): unknown
  version?: string
}

export interface VueServerRendererLike {
  renderToString(app: unknown): Promise<string>
  version?: string
}

let cached: { vue: VueAppLike; server: VueServerRendererLike } | undefined
let attempted = false
// BUG-09: per-adapter load error detail.
let lastLoadError: string | undefined
export function getLastLoadError(): string | undefined {
  return lastLoadError
}

async function load(): Promise<{ vue: VueAppLike; server: VueServerRendererLike } | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const vueMod = (await import('vue')) as unknown as { default?: VueAppLike } & VueAppLike
    const serverMod = (await import('@vue/server-renderer')) as unknown as { default?: VueServerRendererLike } & VueServerRendererLike
    const vue = (vueMod.default ?? vueMod) as VueAppLike
    const server = (serverMod.default ?? serverMod) as VueServerRendererLike
    if (typeof vue.createSSRApp !== 'function') {
      lastLoadError = 'vue imported but createSSRApp is ' + typeof vue.createSSRApp
      return undefined
    }
    if (typeof server.renderToString !== 'function') {
      lastLoadError = '@vue/server-renderer imported but renderToString is ' + typeof server.renderToString
      return undefined
    }
    cached = { vue, server }
    return cached
  } catch (e) {
    lastLoadError = 'import threw: ' + ((e as { message?: string })?.message ?? String(e))
    return undefined
  }
}

/** True when `vue` + `@vue/server-renderer` are resolvable at runtime. */
export async function isVueAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/** The resolved Vue module, or undefined when not installed. */
export async function getVue(): Promise<VueAppLike | undefined> {
  return (await load())?.vue
}

/** The resolved `@vue/server-renderer` module, or undefined when not installed. */
export async function getVueServerRenderer(): Promise<VueServerRendererLike | undefined> {
  return (await load())?.server
}

/**
 * Render a Vue component (via `h`, SFC compiled output, or options object)
 * to HTML through `@vue/server-renderer`. Throws when Vue is absent.
 */
export async function renderVueToString(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const deps = await load()
  if (!deps) {
    throw new Error('@oxide/vue: real SSR requires `vue` and `@vue/server-renderer` to be installed')
  }
  const app = deps.vue.createSSRApp(Component, props)
  return await deps.server.renderToString(app)
}
