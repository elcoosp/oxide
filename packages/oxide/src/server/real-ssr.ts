// Shared scaffolding for FrameworkAdapters that dispatch to a REAL upstream
// framework SSR renderer (spec §7). Each adapter package (react, vue, svelte,
// solid) wraps this helper with its own framework-specific renderBody +
// isAvailable callbacks, so the dispatch logic and shell assembly live once.
//
// Design: the neutral-IR renderer in each adapter's ./render.ts stays the
// DEFAULT (byte-stable, drives the §22 conformance suite). This module powers
// an OPT-IN production path selected by `createRealAdapter({ ssr: 'real' })`,
// which needs two additional inputs from the caller:
//
//   1. `componentFor(route)` — a real framework component for a route.
//   2. The upstream framework deps installed (optional peer deps).
//
// With `ssr: 'auto'`, a missing component or missing deps degrades to the IR
// renderer; with `ssr: 'real'`, a missing input yields an HTML comment so
// the failure is obvious in the response body.
//
// Head merging is OWNED BY THE ADAPTER PACKAGE, not this helper: the adapter
// knows its own layout head (e.g. LAYOUT_HEAD with the "%s · Atlas" template)
// and its own `head(ctx)` implementation. The helper asks for the final
// HeadTags via `cfg.buildHead(input)`, which the adapter composes from its
// own model ⊕ the pipeline-supplied `input.head` (§16.3 merge order).

import type {
  FrameworkAdapter, RenderInput, RenderCtx, HeadTags, ClientInput, Disposer,
} from '../types.ts'

export interface RealAdapterOptions {
  /**
   * Which renderer the adapter uses.
   * - `'auto'` (default): prefer real SSR, fall back to IR when the caller
   *   has no component for a route OR the framework deps are unavailable.
   * - `'real'`: require real SSR; missing inputs render an HTML comment
   *   placeholder instead of falling back (useful for production).
   */
  ssr?: 'real' | 'auto'
  /**
   * Advanced override: resolve a component for a route id without the
   * generated resolver. The normal path is {@link RenderInput.resolve},
   * supplied by the runtime from the build-emitted component surface
   * (§6.4). Kept for one release window for custom component registries.
   */
  componentFor?: (route: string) => unknown
  /** Optional layout: called with the route's tree as children (React-style). */
  layoutFor?: (route: string) => unknown
}

export interface RealAdapterConfig {
  name: string
  extensions: string[]
  jsxImportSource?: string
  capabilities: FrameworkAdapter['capabilities']
  plugins: () => ReturnType<FrameworkAdapter['plugins']>
  /** IR fallback render (from the adapter's ./render.ts). */
  irRender: (input: RenderInput) => ReadableStream<Uint8Array>
  /**
   * Final HeadTags for a RenderInput: the adapter composes its own layout
   * head, `head(ctx)` output, and the pipeline-supplied `input.head`. The
   * helper only emits the resulting model.
   */
  buildHead: (input: RenderInput) => HeadTags
  /** True when the upstream framework deps are resolvable. */
  isAvailable: () => Promise<boolean>
  /** Per-adapter load failure detail (BUG-09): set from the adapter's own
   * `isAvailable()` catch — the generic helper must not read a
   * React-specific global for all four frameworks. */
  lastError?: () => string | undefined
  /** Real SSR body: given a component and props, produce HTML. */
  renderBody: (Component: unknown, props: Record<string, unknown>) => Promise<string>
  /**
   * §7.1 streaming SSR: an optional chunked body renderer. When present, the
   * adapter yields its stream's chunks as the response body — the shell
   * reaches the browser before deferred boundaries resolve. Absent → the
   * adapter uses `renderBody` (one chunk). React wires this to React 19's
   * `renderToReadableStream`; the others can adopt it as their frameworks
   * expose a streaming renderer.
   */
  renderStream?: (Component: unknown, props: Record<string, unknown>) => Promise<ReadableStream<Uint8Array>>
  /** Attributes on the framework root container (e.g. ` id="root" data-reactroot=""`). */
  rootAttrs: string
  /** Client-side entries — re-used from the IR adapter (framework-neutral). */
  hydrate: FrameworkAdapter['hydrate']
  hydrateIsland?: FrameworkAdapter['hydrateIsland']
  mount: FrameworkAdapter['mount']
  update: FrameworkAdapter['update']
  opts: RealAdapterOptions
}

const TEXT_ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;' }
const ATTR_ESCAPES: Record<string, string> = { '&': '&amp;', '"': '&quot;', '<': '&lt;' }
const escText = (s: string): string => s.replace(/[&<>]/g, (c) => TEXT_ESCAPES[c] as string)
const escAttr = (s: string): string => s.replace(/[&"<]/g, (c) => ATTR_ESCAPES[c] as string)

/** Render a neutral HeadTags model to the strings that go inside <head>. */
export function emitHead(head: HeadTags): string {
  const parts: string[] = []
  if (head.title !== undefined) {
    const t = head.titleTemplate && head.titleTemplate.includes('%s')
      ? head.titleTemplate.replace('%s', head.title)
      : head.title
    parts.push(`<title>${escText(t)}</title>`)
  }
  if (head.base) {
    parts.push(`<base href="${escAttr(head.base.href)}"${head.base.target ? ` target="${escAttr(head.base.target)}"` : ''}>`)
  }
  for (const m of head.meta) {
    const attrs = Object.entries(m).map(([k, v]) => ` ${k}="${escAttr(v)}"`).join('')
    parts.push(`<meta${attrs}>`)
  }
  for (const l of head.link) {
    const attrs = Object.entries(l).map(([k, v]) => ` ${k}="${escAttr(v)}"`).join('')
    parts.push(`<link${attrs}>`)
  }
  for (const s of head.style) {
    parts.push(`<style${s.nonce ? ` nonce="${escAttr(s.nonce)}"` : ''}>${s.content}</style>`)
  }
  for (const s of head.script) {
    const attrs = [
      s.src ? ` src="${escAttr(s.src)}"` : '',
      s.type ? ` type="${escAttr(s.type)}"` : '',
      s.nonce ? ` nonce="${escAttr(s.nonce)}"` : '',
      s.defer ? ' defer' : '',
    ].join('')
    parts.push(`<script${attrs}>${s.content ? escText(s.content) : ''}</script>`)
  }
  // §16.3: structured JSON-LD entries are their own script element.
  for (const entry of head.jsonLd ?? []) {
    const body = typeof entry === 'string' ? entry : JSON.stringify(entry)
    parts.push(`<script type="application/ld+json">${escText(body)}</script>`)
  }
  return parts.join('')
}

function streamOf(html: string): ReadableStream<Uint8Array> {
  const enc = new TextEncoder()
  return new ReadableStream<Uint8Array>({
    start(c) { c.enqueue(enc.encode(html)); c.close() },
  })
}

/**
 * Build a FrameworkAdapter whose `render()` dispatches to real upstream SSR
 * when the caller supplies a component and the framework deps are installed,
 * falling back to the neutral-IR renderer otherwise.
 */
export function makeRealAdapter(cfg: RealAdapterConfig): FrameworkAdapter {
  const strategy = cfg.opts.ssr ?? 'auto'
  // §6.4: components come from the runtime resolver; the config's
  // `componentFor` (if any) is an explicit advanced override.
  const resolveComponent = (input: RenderInput, id: string): unknown => {
    const r = (input as RenderInput & { resolve?: (id: string) => unknown }).resolve
    if (typeof r === 'function') return r(id)
    return cfg.opts.componentFor?.(id)
  }
  const missing = (kind: string, id: string): void => {
    console.error(`[oxide] ${cfg.name}: no component resolved for ${kind} \`${id}\``)
  }
  return {
    contract: 1,
    name: cfg.name,
    extensions: cfg.extensions,
    ...(cfg.jsxImportSource ? { jsxImportSource: cfg.jsxImportSource } : {}),
    capabilities: cfg.capabilities,
    plugins: cfg.plugins,
    // head(ctx) is the FrameworkAdapter contract shape; we route it through
    // buildHead so the adapter's merge logic is exercised uniformly by the
    // pipeline whether or not real SSR is used.
    head(ctx: RenderCtx): HeadTags {
      const synthetic: RenderInput = {
        route: ctx.route, url: ctx.url, params: ctx.params, data: ctx.data,
        head: { meta: [], link: [], style: [], script: [] },
        locals: ctx.locals,
      }
      return cfg.buildHead(synthetic)
    },
    render(input: RenderInput): ReadableStream<Uint8Array> {
      const Component = resolveComponent(input, input.route)
      // BUG-09: escape anything interpolated into an HTML comment — a `why`
      // containing `-->` would break out of the comment into live HTML.
      const safeComment = (s: string): string =>
        String(s).replace(/-->/g, '--\\>').replace(/</g, '&lt;')
      // Vue and other frameworks allow component OBJECTS (options API,
      // defineComponent result), not just functions. Only undefined/null
      // means "the caller has no component for this route".
      if (Component === undefined || Component === null) {
        if (strategy === 'real') {
          // §6.4 / §10.2 step 5: a missing page component is a diagnostic and
          // a 500, never a silently blank page. The marker is the adapter
          // status contract the pipeline promotes to HTTP 500.
          missing('route', input.route)
          return streamOf(
            `<!--oxide-status:500--><div class='ox-error' data-ox-error='OX-500'>OX-500: no component resolved for route ${escText(input.route)}</div>`,
          )
        }
        return cfg.irRender(input)
      }
      const enc = new TextEncoder()
      const it = (async function* (): AsyncGenerator<Uint8Array> {
        if (!(await cfg.isAvailable())) {
          if (strategy === 'real') {
            // BUG-09: per-adapter lastError (never a React-specific global),
            // escaped; loud OX-500 page, not a comment-only blank body.
            const why = safeComment(cfg.lastError?.() ?? 'isAvailable=false, no recorded error')
            const head = cfg.buildHead(input)
            yield enc.encode(`<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${emitHead(head)}</head>`)
            yield enc.encode(`<body><div${cfg.rootAttrs}>`)
            yield enc.encode(`<div class='ox-error' data-ox-error='OX-500'>OX-500: ${escText(cfg.name)} SSR unavailable: ${escText(why)}</div>`)
            yield enc.encode(`</div></body></html>`)
            return
          }
          for await (const c of cfg.irRender(input)) yield c
          return
        }
        const head = cfg.buildHead(input)
        // §8.2 / §16.3: nest the page inside the route's layout chain,
        // outermost first. `input.layouts` (populated by the pipeline from
        // the router's `layout_chain`) is the authoritative source; the
        // legacy single-layer `layoutFor(route)` remains a fallback so
        // configs that predate `input.layouts` keep working.
        //
        // The `children: { Component, props }` descriptor is the contract a
        // layout already implements (see the `layoutFor` path below): a
        // layout renders `props.children.Component` with `props.children.props`.
        // Chaining the descriptors bottom-up means each layout sees the
        // next-outer tree as its `children`, so a 3-layout chain renders as
        // L1( L2( L3( page ) ) ) with no new adapter plumbing.
        const layoutIds: string[] = Array.isArray(input.layouts) ? input.layouts : []
        const layoutParts: Array<{ id: string; Component: unknown }> = []
        for (const id of layoutIds) {
          const L = resolveComponent(input, id)
          if (L === undefined || L === null) {
            // §6.4: never silently drop a declared layout. `real` records a
            // diagnostic (and still renders so the page is visible); `auto`
            // keeps the legacy skip.
            if (strategy === 'real') {
              missing('layout', id)
              layoutParts.push({ id, Component: undefined })
            }
            continue
          }
          layoutParts.push({ id, Component: L })
        }
        if (layoutParts.length === 0) {
          const single = cfg.opts.layoutFor?.(input.route)
          // Accept any component shape (function OR object): Vue's
          // `defineComponent` result is an object; the caller's
          // `layoutFor` contract is `unknown` for exactly this reason.
          if (single !== undefined && single !== null) {
            layoutParts.push({ id: `layoutFor(${input.route})`, Component: single })
          }
        }
        const headWarnings: string[] = []
        // §10.2 step 5: `props` is referenced by both the try (page render)
        // and the catch (error.tsx render). Declaring it here keeps it in
        // scope after the try frame unwinds — a `const` inside the try
        // would be `ReferenceError` in the catch handler.
        const props: Record<string, unknown> = {
          route: input.route, url: input.url, params: input.params, data: input.data,
        }
        try {
          // Build the innermost descriptor = the page itself.
          let descriptor: { Component: unknown; props: Record<string, unknown> } = {
            Component, props,
          }
          // §16.3: `input.layouts` is ordered OUTERMOST → INNERMOST. To build
          // the tree we must wrap from the innermost layout outward, so we
          // walk the list in reverse; the final assignment is the outermost
          // (element 0), yielding L1(L2(L3(page))) when rendered.
          for (const layer of [...layoutParts].reverse()) {
            if (layer.Component === undefined) {
              headWarnings.push(
                `<!-- oxide: layout ${safeComment(layer.id)} declared but the resolver returned undefined; skipped -->`,
              )
              continue
            }
            descriptor = {
              Component: layer.Component,
              props: { ...props, children: descriptor },
            }
          }
          yield enc.encode(`<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${emitHead(head)}</head>`)
          if (headWarnings.length > 0) {
            yield enc.encode(`<body>${headWarnings.join('')}<div${cfg.rootAttrs}>`)
          } else {
            yield enc.encode(`<body><div${cfg.rootAttrs}>`)
          }
          // §7.1: when the adapter supplies a streaming body renderer, pump
          // its chunks through as they arrive. The shell bytes above have
          // already been yielded, so the browser starts parsing before the
          // component tree finishes rendering. `renderBody` (one chunk) is
          // the fallback for adapters/frameworks without a streaming API.
          if (typeof cfg.renderStream === 'function') {
            const bodyStream = await cfg.renderStream(descriptor.Component, descriptor.props)
            const reader = bodyStream.getReader()
            for (;;) {
              const { value, done } = await reader.read()
              if (done) break
              if (value && value.length > 0) yield value
            }
          } else {
            const body = await cfg.renderBody(descriptor.Component, descriptor.props)
            yield enc.encode(body)
          }
          yield enc.encode('</div></body></html>')
        } catch (err) {
          // §10.2 step 5: prefer the project's error.tsx over the generic
          // OX-500 placeholder. The pipeline hands `input.errorRoute` (nearest
          // `error.tsx` ancestor id) and the adapter tries that component
          // first; if it also throws, or none is registered, fall through to
          // the generic text.
          const msg = err instanceof Error ? err.message : String(err)
          const errId = input.errorRoute
          if (typeof errId === 'string' && errId.length > 0) {
            const ErrComp = resolveComponent(input, errId)
            if (ErrComp !== undefined && ErrComp !== null) {
              try {
                const errProps: Record<string, unknown> = {
                  ...props,
                  error: {
                    message: msg,
                    route: input.route,
                    status: 500,
                  },
                }
                const body = await cfg.renderBody(ErrComp, errProps)
                // §10.2 step 5 (error-status contract): announce the error
                // render in the FIRST bytes of the replacement document so
                // the pipeline's first-chunk probe can fix the HTTP status
                // to 500 before the response head is written. When the
                // throw happened before the shell yielded (descriptor
                // build, componentFor, render setup), this document IS the
                // first chunk and the probe observes the marker; a
                // late-throw after the shell has already streamed keeps
                // the original status (HTTP cannot retroactively change
                // it — the pipeline documents this deviation).
                yield enc.encode(`<!--oxide-status:500--><!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${emitHead(head)}</head>`)
                yield enc.encode(`<body><div${cfg.rootAttrs}>${body}</div></body></html>`)
                return
              } catch {
                // The error page itself threw — fall through to the generic
                // OX-500 text below, so a broken error.tsx does not blank
                // the response.
              }
            }
          }
          // §10.2 step 5: the canonical marker first (see the error.tsx
          // branch above) so the generic fallback also produces a 500 when
          // it forms the response's first chunk; the OX-500 boundary
          // attribute remains the pipeline's secondary signal.
          yield enc.encode(`<!--oxide-status:500--><!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${emitHead(head)}</head>`)
          yield enc.encode(`<body><div${cfg.rootAttrs}>`)
          yield enc.encode(`<div class='ox-error' data-ox-error='OX-500'>OX-500: ${escText(msg)}</div>`)
          yield enc.encode(`</div></body></html>`)
        }
      })()
      return new ReadableStream<Uint8Array>({
        async pull(c) {
          try {
            const { value, done } = await it.next()
            if (done) c.close()
            else c.enqueue(value)
          } catch (err) { c.error(err) }
        },
      })
    },
    hydrate: cfg.hydrate,
    ...(cfg.hydrateIsland ? { hydrateIsland: cfg.hydrateIsland } : {}),
    mount: cfg.mount,
    update: cfg.update,
  }
}

export type { ClientInput, Disposer }
