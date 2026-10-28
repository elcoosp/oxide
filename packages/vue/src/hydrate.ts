// @oxide/vue client entry (spec §7 client fns): hydrate / hydrateIsland /
// mount / update. Without @vue/runtime-core mounted the adapter replays its
// own deterministic markup and binds event handlers onto the `data-ox-id`
// protocol markers the server renderer emitted (see src/render.ts).
//
// All DOM access goes through injectable params (`doc`) so tests drive the
// client fns against a hand-rolled fake DOM (test/helpers/fake-dom.ts) — the
// same pattern as the packages/oxide client modules (§8.4/§10.3).
//
// The defer glue (§7.1) is the CORE's (packages/oxide/src/client/defer-glue),
// imported by relative path so every adapter shares one implementation: the
// swapper script dispatches `ox:swap` and this glue resolves the matching
// DeferredClient slots.

import { attachDeferGlue, type DeferGlue } from '@oxide-fw/core/client/defer-glue'
import { componentFor, getRouteComponentSource, routeIdFor } from '@oxide-fw/core/client/route-manifest'
import type { ClientInput, Disposer } from '@oxide-fw/core/types'
import { el, type ComponentFn, type Node } from './ir.ts'
import { renderShellToString, renderNodeToString, walkTreeForHandlers, newRenderState, pageComponent, type HandlerBinding } from './render.ts'
import { mountVuePageRoot, type RealPageRoot } from './real-client.ts'

// ---------------------------------------------------------------------------
// Injectable DOM surfaces (structural — real elements and the fake DOM both fit)
// ---------------------------------------------------------------------------

export interface OxElement {
  getAttribute(name: string): string | null
  setAttribute(name: string, value: string): void
  hasAttribute(name: string): boolean
  addEventListener(type: string, fn: (ev: unknown) => void): void
  removeEventListener(type: string, fn: (ev: unknown) => void): void
  dispatchEvent?(ev: unknown): boolean
  innerHTML: string
  /** The DOM specification types this as `Document | null` (null when the
   *  element is detached from its document), not `Document | undefined`.
   *  Widening to `OxDoc | null` makes OxElement DOM-faithful and lets a
   *  fake DOM in tests structurally satisfy the type without a cast. */
  ownerDocument?: OxDoc | null
  querySelector?(sel: string): OxElement | null
  querySelectorAll?(sel: string): ArrayLike<OxElement>
}

export interface OxDoc {
  addEventListener(type: string, fn: (ev: unknown) => void): void
  removeEventListener(type: string, fn: (ev: unknown) => void): void
  dispatchEvent?(ev: unknown): boolean
  querySelector?(sel: string): OxElement | null
  querySelectorAll?(sel: string): ArrayLike<OxElement>
}

export interface ClientOptions {
  doc?: OxDoc
  /** Navigation hook (wired to the core router in the bootstrap, §8.4). */
  navigate?: (href: string) => void
  /** Called for every ox:swap — adapter hydration of swapped content (§7.1). */
  onSwap?: (detail: { id: number }) => void
}

interface ClientHooks {
  /** Re-render the current route tree with patched data (adapter.update). */
  update(patch: Record<string, unknown>): void
  /** Client navigation hook (the core router intercepts links first, §8.4). */
  navigate(href: string): void
}

interface ClientHandle {
  update(input: ClientInput): void
  dispose(): void
}

// ---------------------------------------------------------------------------
// Tree building (client side of the shared deterministic page registry)
// ---------------------------------------------------------------------------

type PageProps = {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: Record<string, unknown>
  client?: ClientHooks
}

function pageTreeFor(input: ClientInput, client: ClientHooks): Node {
  const page = pageComponent(input.route)
  const props: PageProps = {
    route: input.route,
    url: input.url,
    params: input.params,
    data: input.data as Record<string, unknown>,
    client,
  }
  return el(page, props)
}

function pageRenderInput(input: ClientInput): Parameters<typeof renderShellToString>[0] {
  return {
    route: input.route,
    url: input.url,
    params: input.params,
    data: input.data,
    head: { meta: [], link: [], style: [], script: [] },
  }
}

/** Find the element carrying `data-ox-id='<id>'` under `scope` (or itself). */
function findMarked(scope: OxElement, id: string): OxElement | null {
  if (scope.getAttribute('data-ox-id') === id) return scope
  return scope.querySelector?.(`[data-ox-id="${id}"]`) ?? null
}

/** Bind a rebuilt tree's handler markers onto the live DOM. */
function bindAll(
  scope: OxElement,
  tree: Node,
  bound: Array<{ el: OxElement; type: string; fn: (ev: unknown) => void }>,
): void {
  for (const binding of walkTreeForHandlers(tree)) {
    const target = findMarked(scope, binding.id)
    if (!target) continue
    target.addEventListener(binding.event, binding.handler)
    bound.push({ el: target, type: binding.event, fn: binding.handler })
  }
}

// ---------------------------------------------------------------------------
// Client root (hydrate / mount / update)
// ---------------------------------------------------------------------------

/**
 * Client-side singleton: the contract's `update(input)` has no root parameter,
 * so the last hydrate()/mount() target is tracked here. Client-only state —
 * the server keeps NO module-global request state (§7.2).
 */
let active: ClientHandle | undefined

/**
 * §8.4 / §10.3: true when the registered client manifest resolves this route
 * to a client chunk — the real-render path owns the page then (the caller
 * skips the IR paint and lets the async manifest render land). The manifest
 * source is registered by `bootWithRouter` from `/client/client.manifest.json`.
 */
function manifestRouteExists(route: string): boolean {
  const src = getRouteComponentSource()
  return src !== undefined && routeIdFor(route, src.manifest) !== undefined
}

/** Narrow an `OxElement` to a real DOM `Element`.
 *
 * `OxElement` is a structural subset of the DOM `Element` interface — the
 * minimum a fake DOM in tests must implement. Real framework runtime calls
 * (`createRoot`, `createSSRApp().mount`, etc.) require the full `Element`.
 * This helper is a *runtime assertion*, not a cast: `instanceof Element`
 * succeeds in a real browser, throws a clear TypeError otherwise. In the
 * test environment the real-adapter path is never reached (the framework
 * isn't installed → `mountXxxPageRoot` returns undefined before this runs).
 */
function asDomElement(v: OxElement, where: string): Element {
  if (typeof Element !== 'undefined' && v instanceof Element) return v
  throw new TypeError(`oxide: ${where} requires a live DOM Element`)
}

function createClientRoot(root: OxElement, input: ClientInput, doc: OxDoc, opts: ClientOptions): ClientHandle {
  let current = input
  let glue: DeferGlue | undefined
  let disposed = false
  // §8.4 manifest path: the live real Vue app + the navigation sequence
  // guard (async chunk imports can resolve out of order — an older render
  // must never paint over a newer one) + the serialized chain (two
  // navigations racing on the same container mount one after the other,
  // never concurrently).
  let realRoot: RealPageRoot | undefined
  let renderSeq = 0
  let realChain: Promise<void> = Promise.resolve()
  const bound: Array<{ el: OxElement; type: string; fn: (ev: unknown) => void }> = []

  const client: ClientHooks = {
    update(patch: Record<string, unknown>): void {
      update({ ...current, data: { ...(current.data as Record<string, unknown>), ...patch } })
    },
    navigate(href: string): void {
      if (opts.navigate) opts.navigate(href)
      else doc.dispatchEvent?.({ type: 'ox:navigate', detail: { href } })
    },
  }

  function unbind(): void {
    for (const b of bound) b.el.removeEventListener(b.type, b.fn)
    bound.length = 0
  }

  function rebind(): void {
    unbind()
    // Rebuild the deterministic tree; bind onto the DOM elements the server
    // marked with data-ox-on / data-ox-id (same walk ⇒ same marker ids).
    bindAll(root, pageTreeFor(current, client), bound)
  }

  function render(inp: ClientInput): void {
    // Client re-render uses the shell renderer (deterministic markup, §7.2);
    // streamed deferred slots are owned by the server stream + ox:swap glue.
    root.innerHTML = renderShellToString(pageRenderInput(inp))
  }

  /** The props a REAL page component receives — the same shape the server's
   * real SSR path (`makeRealAdapter`) renders with, so hydration matches. */
  function realProps(inp: ClientInput): Record<string, unknown> {
    return { route: inp.route, url: inp.url, params: inp.params, data: inp.data }
  }

  /** Synchronous IR fallback paint (also the real path's failure mode). */
  function renderIr(): void {
    render(current)
    rebind()
  }

  /**
   * §8.4 + §10.3: the manifest-driven real render. Resolves the route's page
   * component from the client manifest (`componentFor` → dynamic import of
   * the route's `entry_chunk`), then mounts/hydrates it with Vue's own
   * runtime via `mountVuePageRoot`. Returns `false` immediately when no
   * manifest source is registered or the route isn't in it — the caller
   * then takes the IR fallback synchronously. When it returns `true` the
   * async chain owns the root; a missing component, a broken chunk or an
   * absent Vue falls back to the IR paint inside the chain (sequence-
   * guarded, so a superseded navigation never paints).
   */
  function tryRealRender(seq: number, inp: ClientInput): boolean {
    const src = getRouteComponentSource()
    if (!src) return false
    if (routeIdFor(inp.route, src.manifest) === undefined) return false
    if (seq !== renderSeq) return true
    // §7.1 / §9.2: wire deferred markers BEFORE the tree paints so the
    // components receive promise-likes, not `{__deferred: id}` placeholders.
    glue?.wire(current.data)
    realChain = realChain.then(async () => {
      if (disposed || seq !== renderSeq) return
      const Component = await componentFor(inp.route)
      if (disposed || seq !== renderSeq) return
      if (Component === undefined || Component === null) {
        renderIr()
        return
      }
      try {
        if (realRoot) {
          realRoot.update(realProps(inp))
          return
        }
        // First real mount: hydrate the SSR markup when present, fresh-mount
        // into an empty root (SPA mode).
        const mounted = await mountVuePageRoot(
          asDomElement(root, 'real framework mount'),
          Component,
          realProps(inp),
          { hydrate: (root.innerHTML ?? '') !== '' },
        )
        if (disposed || seq !== renderSeq) {
          mounted?.dispose()
          return
        }
        if (!mounted) {
          // vue not installed → IR fallback.
          renderIr()
          return
        }
        realRoot = mounted
      } catch (e) {
        console.warn?.('oxide: real page render failed; falling back to IR', e)
        if (!disposed && seq === renderSeq) renderIr()
      }
    })
    return true
  }

  function update(inp: ClientInput): void {
    current = inp
    const seq = ++renderSeq
    if (tryRealRender(seq, inp)) return
    // §7.1 / §9.2: re-wire the new page's deferred markers on navigation.
    glue?.wire(inp.data)
    // §7.1 / §9.2: `/__data` carries the loader's already-resolved defer
    // values; settle the matching slots so a component awaiting one gets
    // its value (the same contract SSR's deferred script provides).
    if (inp.deferred) glue?.resolveAll(inp.deferred)
    render(inp)
    rebind()
  }

  // Hydration: the server markup is already in the root — do NOT rewrite it
  // (deterministic markup ⇒ no hydration mismatch, §7.2). Only bind. An empty
  // root (defensive) falls back to a client render. When a manifest route
  // exists the real render owns the root instead (the IR paint is skipped;
  // the chain's IR fallback repaints if it fails).
  glue = attachDeferGlue({ doc, onSwap: (d) => opts.onSwap?.(d) })
  // §7.1 / §9.2: rewrite `{__deferred: id}` markers in the loader data into
  // `DeferredClient` slots so the adapter's components can await them.
  const seq = ++renderSeq
  if (!tryRealRender(seq, current)) {
    if (root.innerHTML === '') render(current)
    rebind()
    glue.wire(current.data)
  }

  const handle: ClientHandle = {
    update,
    dispose(): void {
      if (disposed) return
      disposed = true
      unbind()
      realRoot?.dispose()
      realRoot = undefined
      glue?.dispose()
      glue = undefined
      if (active === handle) active = undefined
    },
  }
  return handle
}

// ---------------------------------------------------------------------------
// Public client API (§7)
// ---------------------------------------------------------------------------

/**
 * Hydrate server-rendered markup: binds handlers + the core defer glue while
 * preserving the server HTML byte-for-byte (no mismatch, §7.2).
 */
export function hydrate(root: OxElement, input: ClientInput, opts: ClientOptions = {}): Disposer {
  const doc = opts.doc ?? root.ownerDocument
  if (!doc) throw new TypeError('hydrate: no document (pass opts.doc in non-browser tests)')
  const handle = createClientRoot(root, input, doc, opts)
  active = handle
  return handle.dispose
}

/** SPA mode (§10.1 `spa`): render the shell into an empty root, then bind.
 * When a manifest route exists the real page component paints instead —
 * `createClientRoot` skips the IR shell for that route (§8.4/§10.3). */
export function mount(root: OxElement, input: ClientInput, opts: ClientOptions = {}): Disposer {
  const doc = opts.doc ?? root.ownerDocument
  if (!doc) throw new TypeError('mount: no document (pass opts.doc in non-browser tests)')
  if (!manifestRouteExists(input.route)) {
    root.innerHTML = renderShellToString(pageRenderInput(input))
  }
  const handle = createClientRoot(root, input, doc, opts)
  active = handle
  return handle.dispose
}

/**
 * Client navigation (§7 update): swap the route tree in the active root.
 * No-op when no client root is mounted (e.g. update fires before mount).
 */
export function update(input: ClientInput): void {
  active?.update(input)
}

/**
 * Islands hydration (§10.3): the core loader schedules by strategy and calls
 * this with the loaded chunk module. The module's default component renders
 * into the host (deterministic markup) and its handlers bind to the
 * data-ox-id markers — one rendering path shared with full-page hydration.
 */
export function hydrateIsland(host: OxElement, mod: unknown, props: unknown, opts: ClientOptions = {}): Disposer {
  const component = (mod as { default?: ComponentFn }).default ?? (mod as ComponentFn)
  if (typeof component !== 'function') {
    throw new TypeError('hydrateIsland: island module must export a default component')
  }
  const bound: Array<{ el: OxElement; type: string; fn: (ev: unknown) => void }> = []
  const client: ClientHooks = {
    update(patch: Record<string, unknown>): void {
      const nextProps = { ...(props as Record<string, unknown>), ...patch, client }
      const tree = component(nextProps)
      host.innerHTML = renderNodeToString(tree, islandRenderState())
      bindAll(host, tree, bound)
    },
    navigate(href: string): void {
      opts.navigate?.(href)
    },
  }
  const tree = component({ ...(props as Record<string, unknown>), client })
  host.innerHTML = renderNodeToString(tree, islandRenderState())
  bindAll(host, tree, bound)
  return () => {
    for (const b of bound) b.el.removeEventListener(b.type, b.fn)
    bound.length = 0
  }
}

function islandRenderState(): ReturnType<typeof newRenderState> {
  return newRenderState('routes/islands')
}

export type { ClientHandle, HandlerBinding }
