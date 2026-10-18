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

import { attachDeferGlue, type DeferGlue } from '../../oxide/src/client/defer-glue.ts'
import type { ClientInput, Disposer } from '../../oxide/src/types.ts'
import { el, type ComponentFn, type Node } from './ir.ts'
import { renderShellToString, renderNodeToString, walkTreeForHandlers, newRenderState, pageComponent, type HandlerBinding } from './render.ts'

// ---------------------------------------------------------------------------
// Injectable DOM surfaces (structural — real elements and the fake DOM both fit)
// ---------------------------------------------------------------------------

export interface OxElement {
  getAttribute(name: string): string | null
  setAttribute(name: string, value: string): void
  hasAttribute(name: string): boolean
  addEventListener(type: string, fn: (ev: unknown) => void): void
  removeEventListener(type: string, fn: (ev: unknown) => void): void
  dispatchEvent?(ev: { type: string; detail?: unknown }): boolean
  innerHTML: string
  ownerDocument?: OxDoc
  querySelector?(sel: string): OxElement | null
  querySelectorAll?(sel: string): ArrayLike<OxElement>
}

export interface OxDoc {
  addEventListener(type: string, fn: (ev: unknown) => void): void
  removeEventListener(type: string, fn: (ev: unknown) => void): void
  dispatchEvent?(ev: { type: string; detail?: unknown }): boolean
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

interface PageProps {
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
  return el(page, props as unknown as Record<string, unknown>)
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

function createClientRoot(root: OxElement, input: ClientInput, doc: OxDoc, opts: ClientOptions): ClientHandle {
  let current = input
  let glue: DeferGlue | undefined
  let disposed = false
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

  function update(inp: ClientInput): void {
    current = inp
    render(inp)
    rebind()
    // §7.1 / §9.2: re-wire the new page's deferred markers on navigation.
    glue?.wire(inp.data)
  }

  // Hydration: the server markup is already in the root — do NOT rewrite it
  // (deterministic markup ⇒ no hydration mismatch, §7.2). Only bind. An empty
  // root (defensive) falls back to a client render.
  if (root.innerHTML === '') render(current)
  rebind()
  glue = attachDeferGlue({ doc: doc as unknown as Document, onSwap: (d) => opts.onSwap?.(d) })
  // §7.1 / §9.2: rewrite `{__deferred: id}` markers in the loader data into
  // `DeferredClient` slots so the adapter's components can await them.
  glue.wire(current.data)

  const handle: ClientHandle = {
    update,
    dispose(): void {
      if (disposed) return
      disposed = true
      unbind()
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

/** SPA mode (§10.1 `spa`): render the shell into an empty root, then bind. */
export function mount(root: OxElement, input: ClientInput, opts: ClientOptions = {}): Disposer {
  const doc = opts.doc ?? root.ownerDocument
  if (!doc) throw new TypeError('mount: no document (pass opts.doc in non-browser tests)')
  root.innerHTML = renderShellToString(pageRenderInput(input))
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
