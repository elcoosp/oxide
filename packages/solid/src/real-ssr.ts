// @oxide/solid real Solid SSR path (spec §7).
//
// The IR-based renderer in ./render.ts is the NEUTRAL fallback. This module
// is the REAL path: it dynamically imports `@solidjs/web` and calls the
// upstream renderer.
//
// Solid 2.0 (RC) moved the web renderer out of `solid-js/web` into the
// dedicated `@solidjs/web` package, and REMOVED `renderToStringAsync`: async
// work is now native to the reactive graph, so a single `renderToString`
// handles Suspense/resources/components awaiting on async data. Solid 1.x
// (`solid-js/web`) is still supported when `@solidjs/web` is not installed —
// the loader checks both packages and normalizes the API surface.
//
// Imports are dynamic and cached so the package keeps working (falling back
// to the IR renderer) when Solid is absent.

export interface SolidWebLike {
  /**
   * Solid 2.0+ renderer — async is native to the reactive graph, so this
   * single entry handles everything (sync trees, resources, Suspense). May
   * return a string OR a Promise depending on the input tree.
   */
  renderToString?(fn: () => unknown): string | Promise<string>
  /**
   * Solid 1.x async renderer. Removed in 2.0; kept optional so the same
   * loader works against either major version.
   */
  renderToStringAsync?(fn: () => unknown): Promise<string>
  ssrElement?(tag: string, attrs: Record<string, unknown>, children: unknown, dynamic: boolean): unknown
  escape?(s: string, attr?: boolean): string
  version?: string
}

interface Loaded {
  web: SolidWebLike
  /** Which package/API resolved — surfaced for error messages + tests. */
  source: 'solid-js/web' | '@solidjs/web'
}

let cached: Loaded | undefined
let attempted = false
// BUG-09: per-adapter load error detail.
let lastLoadError: string | undefined
export function getLastLoadError(): string | undefined {
  return lastLoadError
}

async function tryLoad(name: 'solid-js/web' | '@solidjs/web'): Promise<SolidWebLike | undefined> {
  try {
    const mod = (await import(name)) as unknown as SolidWebLike
    const hasRenderer =
      typeof mod.renderToString === 'function' ||
      typeof mod.renderToStringAsync === 'function'
    if (!hasRenderer) {
      lastLoadError = `${name} imported but has no renderToString/renderToStringAsync`
      return undefined
    }
    return mod
  } catch (e) {
    lastLoadError = `import ${name} threw: ` + ((e as { message?: string })?.message ?? String(e))
    return undefined
  }
}

async function load(): Promise<Loaded | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  // Prefer the dedicated Solid 2.0 package when installed; fall back to the
  // Solid 1.x location so old projects keep working.
  const web2 = await tryLoad('@solidjs/web')
  if (web2) {
    cached = { web: web2, source: '@solidjs/web' }
    return cached
  }
  const web1 = await tryLoad('solid-js/web')
  if (web1) {
    cached = { web: web1, source: 'solid-js/web' }
    return cached
  }
  return undefined
}

/** True when the Solid renderer (either major) is resolvable at runtime. */
export async function isSolidAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/** The resolved Solid web module, or undefined when not installed. */
export async function getSolidWeb(): Promise<SolidWebLike | undefined> {
  return (await load())?.web
}

/** Which package was resolved (`@solidjs/web` for 2.0, `solid-js/web` for 1.x). */
export async function getSolidSource(): Promise<'solid-js/web' | '@solidjs/web' | undefined> {
  return (await load())?.source
}

/**
 * Render a Solid component factory to HTML via the upstream renderer. Uses
 * `renderToStringAsync` when present (Solid 1.x) and `renderToString`
 * otherwise (Solid 2.0, where async is native). The factory is called with
 * no arguments and must return the root element (typically a JSX-compiled
 * tree, or a value built via `ssrElement`).
 */
export async function renderSolidToString(fn: () => unknown): Promise<string> {
  const loaded = await load()
  if (!loaded) {
    throw new Error('@oxide/solid: real SSR requires `@solidjs/web` (Solid 2) or `solid-js/web` (Solid 1) to be installed')
  }
  const web = loaded.web
  if (typeof web.renderToStringAsync === 'function') {
    return await web.renderToStringAsync(fn)
  }
  if (typeof web.renderToString === 'function') {
    const out = web.renderToString(fn)
    // Solid 2.0's renderToString may return a Promise directly (async is
    // native). Awaiting a string is a no-op; this keeps callers uniform.
    return typeof out === 'string' ? out : await out
  }
  throw new Error('@oxide/solid: upstream renderer exposes neither renderToString nor renderToStringAsync')
}

/**
 * Stream a Solid component's HTML via `@solidjs/web`'s `renderToStream`.
 * Yields the shell before Suspense/resources resolve. Falls back to a
 * single-chunk stream from `renderSolidToString` when the runtime lacks
 * `renderToStream` (Solid 1.x's web runtime exposes only `renderToString`).
 *
 * `renderToStream` takes a zero-argument factory (like `renderToString`).
 */
export async function renderSolidToStream(
  fn: () => unknown,
): Promise<ReadableStream<Uint8Array>> {
  const loaded = await load()
  if (!loaded) {
    throw new Error('@oxide/solid: real SSR requires `@solidjs/web` (Solid 2) or `solid-js/web` (Solid 1) to be installed')
  }
  const web = loaded.web as unknown as {
    renderToStream?: (fn: () => unknown) => Promise<NodeJS.ReadableStream | ReadableStream<Uint8Array>>
    renderToString?: (fn: () => unknown) => string | Promise<string>
  }
  if (typeof web.renderToStream === 'function') {
    const solidStream = await web.renderToStream(fn)
    if (typeof (solidStream as ReadableStream<Uint8Array>).getReader === 'function') {
      return solidStream as ReadableStream<Uint8Array>
    }
    const nodeStream = solidStream as NodeJS.ReadableStream
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
  // Fallback: single-chunk from renderToString.
  const html = await renderSolidToString(fn)
  const bytes = new TextEncoder().encode(html)
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}

/**
 * Stream a Solid component's HTML via `@solidjs/web`'s `renderToStream`.
 * Yields the shell before Suspense/resources resolve. Falls back to a
 * single-chunk stream from `renderSolidToString` when the runtime lacks
 * `renderToStream` (Solid 1.x's web runtime exposes only `renderToString`).
 *
 * `renderToStream` takes a zero-argument factory (like `renderToString`).
 */
export async function renderSolidToStream(
  fn: () => unknown,
): Promise<ReadableStream<Uint8Array>> {
  const loaded = await load()
  if (!loaded) {
    throw new Error('@oxide/solid: real SSR requires `@solidjs/web` (Solid 2) or `solid-js/web` (Solid 1) to be installed')
  }
  const web = loaded.web as unknown as {
    renderToStream?: (fn: () => unknown) => Promise<NodeJS.ReadableStream | ReadableStream<Uint8Array>>
    renderToString?: (fn: () => unknown) => string | Promise<string>
  }
  if (typeof web.renderToStream === 'function') {
    const solidStream = await web.renderToStream(fn)
    if (typeof (solidStream as ReadableStream<Uint8Array>).getReader === 'function') {
      return solidStream as ReadableStream<Uint8Array>
    }
    const nodeStream = solidStream as NodeJS.ReadableStream
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
  // Fallback: single-chunk from renderToString.
  const html = await renderSolidToString(fn)
  const bytes = new TextEncoder().encode(html)
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}
