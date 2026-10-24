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
  renderToStream?(app: unknown): Promise<NodeJS.ReadableStream | ReadableStream<Uint8Array>>
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

/**
 * Structural shape of a layout child descriptor (§8.2). `makeRealAdapter`
 * hands a layout a `children` value of `{ Component, props }` — a plain
 * descriptor, not a live vnode, so the layout chain stays framework-
 * agnostic on the wire. Vue consumes children through SLOTS, so the
 * expansion wraps each layer with `h(L, props, { default: () => child })`.
 */
interface ChildDescriptor {
  Component: unknown
  props: Record<string, unknown>
}

function isChildDescriptor(v: unknown): v is ChildDescriptor {
  return (
    typeof v === 'object' &&
    v !== null &&
    'Component' in v &&
    'props' in v &&
    typeof (v as { Component: unknown }).Component !== 'undefined'
  )
}

/**
 * §8.2: expand a layout child descriptor chain into a real vnode, depth-
 * first — the innermost page becomes `h(page, pageProps)` first, then each
 * outer layout wraps it as DEFAULT SLOT content:
 * `h(L, layoutProps, { default: () => childVnode })`. The descriptor is
 * removed from the layout's props (a Vue layout consumes children via
 * `<slot />`, not a prop). A page with no children renders as a plain
 * `h(Component, props)`.
 */
function expandDescriptor(vue: VueAppLike, descriptor: ChildDescriptor): unknown {
  const props: Record<string, unknown> = { ...descriptor.props }
  if (!isChildDescriptor(props.children)) {
    return vue.h(descriptor.Component, props)
  }
  const child = expandDescriptor(vue, props.children)
  delete props.children
  return vue.h(descriptor.Component, props, { default: () => child })
}

/**
 * Build the SSR app for a component + props. When `props.children` is a
 * layout child descriptor, the descriptor chain is expanded into the slot
 * pattern and the resulting vnode is rendered as the app root through a
 * render-closure component (a valid Vue root). Otherwise the component is
 * used directly with the props — the pre-descriptor behavior.
 */
function vueAppFor(
  vue: VueAppLike,
  Component: unknown,
  props: Record<string, unknown>,
): unknown {
  if (isChildDescriptor(props.children)) {
    const vnode = expandDescriptor(vue, { Component, props })
    return vue.createSSRApp({ render: () => vnode })
  }
  return vue.createSSRApp(Component, props)
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
 * to HTML through `@vue/server-renderer`. A layout child descriptor
 * (`props.children = { Component, props }`, §8.2) is expanded into the slot
 * pattern first — see {@link expandDescriptor}. Throws when Vue is absent.
 */
export async function renderVueToString(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const deps = await load()
  if (!deps) {
    throw new Error('@oxide/vue: real SSR requires `vue` and `@vue/server-renderer` to be installed')
  }
  const app = vueAppFor(deps.vue, Component, props)
  return await deps.server.renderToString(app)
}

/**
 * Stream a Vue component's HTML via `@vue/server-renderer`'s `renderToStream`.
 * Yields the shell before async setup/suspense resolves — the TTFB property
 * streaming exists for. Falls back to a single-chunk stream from
 * `renderVueToString` when the runtime lacks `renderToStream`.
 */
export async function renderVueToStream(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<ReadableStream<Uint8Array>> {
  const deps = await load()
  if (!deps) {
    throw new Error('@oxide/vue: real SSR requires `vue` and `@vue/server-renderer` to be installed')
  }
  // Same descriptor expansion as `renderVueToString`: a layout receives
  // `props.children` as `{ Component, props }`; Vue needs slot content.
  const app = vueAppFor(deps.vue, Component, props)
  const server = deps.server
  if (typeof server.renderToStream === 'function') {
    const vueStream = await server.renderToStream(app)
    // `renderToStream` returns a Node Readable on Node and a web
    // ReadableStream on edge runtimes. Normalize to a web stream — the
    // worker consumes `for await` which works on both, but the adapter's
    // contract is a web `ReadableStream`.
    if (typeof (vueStream as ReadableStream<Uint8Array>).getReader === 'function') {
      return vueStream as ReadableStream<Uint8Array>
    }
    // Node Readable → web ReadableStream. Guard `on` — an unknown stream
    // shape (a test fake, a custom runtime) has neither `getReader` nor `on`;
    // fall through to the `renderToString` fallback below so the caller still
    // gets valid HTML (only the incremental chunking is lost).
    const nodeStream = vueStream as NodeJS.ReadableStream
    if (typeof (nodeStream as { on?: unknown }).on === 'function') {
      return new ReadableStream<Uint8Array>({
        start(controller) {
          nodeStream.on('data', (chunk: Buffer | string) => {
            controller.enqueue(typeof chunk === 'string' ? new TextEncoder().encode(chunk) : new Uint8Array(chunk))
          })
          nodeStream.on('end', () => controller.close())
          nodeStream.on('error', (e) => controller.error(e))
        },
      })
    }
  }
  // No streaming API: single-chunk fallback.
  const html = await server.renderToString(app)
  const bytes = new TextEncoder().encode(html)
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}
