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
  /** Resolve the real framework component for a route id. */
  componentFor: (route: string) => unknown
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
  /** Real SSR body: given a component and props, produce HTML. */
  renderBody: (Component: unknown, props: Record<string, unknown>) => Promise<string>
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
      const Component = cfg.opts.componentFor(input.route)
      if (typeof Component !== 'function') {
        if (strategy === 'real') {
          return streamOf(`<!-- oxide: real SSR: no component for route ${input.route} -->`)
        }
        return cfg.irRender(input)
      }
      const enc = new TextEncoder()
      const it = (async function* (): AsyncGenerator<Uint8Array> {
        if (!(await cfg.isAvailable())) {
          if (strategy === 'real') {
            yield enc.encode(`<!-- oxide: real SSR unavailable: ${cfg.name} deps not installed -->`)
            return
          }
          for await (const c of cfg.irRender(input)) yield c
          return
        }
        const head = cfg.buildHead(input)
        try {
          const props: Record<string, unknown> = {
            route: input.route, url: input.url, params: input.params, data: input.data,
          }
          const layout = cfg.opts.layoutFor?.(input.route)
          const body = typeof layout === 'function'
            ? await cfg.renderBody(layout, { ...props, children: { Component, props } })
            : await cfg.renderBody(Component, props)
          yield enc.encode(`<!doctype html>\n<html lang="en"><head>${emitHead(head)}</head>`)
          yield enc.encode(`<body><div${cfg.rootAttrs}>${body}</div></body></html>`)
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          yield enc.encode(`<!doctype html>\n<html lang="en"><head>${emitHead(head)}</head>`)
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
