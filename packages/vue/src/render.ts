// @oxide/vue server renderer (spec §7 + §7.1 + §10.2).
//
// Without @vue/server-renderer installed, the adapter renders the neutral
// element-tree IR itself: components are plain functions rendered
// recursively. Vue's SSR conventions applied here:
//   - the app container carries `data-v-app` (createApp mount convention);
//   - every component element carries the scoped-CSS attribute
//     `data-v-<scope>` (scope id = deterministic hash of the route);
//   - suspense is `core` (§7): Vue's streaming is partial, so the core owns
//     the deferred boundaries — the IR's Defer node emits
//     `template[data-ox-b]`/`[data-ox-c]` wire shapes IDENTICAL to the
//     render crate so the conformance suite (§22) passes for every adapter.
//
// The Atlas page tree (spec §22 reference app) is built in below: the
// conformance suite renders these routes through every adapter.

import type { RenderInput, RenderCtx, HeadTags } from '../../oxide/src/types.ts'
import { encodePropsB64 } from '../../oxide/src/odf-encode.ts'
import {
  el, t, raw, frag, deferred, island, deferPromise, escapeText, escapeAttr, hash6, toBase64,
  type Node, type ComponentFn, type DeferSource,
} from './ir.ts'
import { boundaryFallback, boundaryContent, islandElement, errorBoundary, hydrationBootstrap } from './protocol.ts'

// ---------------------------------------------------------------------------
// Render context + flavor
// ---------------------------------------------------------------------------

interface RenderState {
  /** Deferred boundaries collected during the shell walk (§7.1). */
  defers: Array<{ id: number; value: DeferSource; render: (resolved: unknown) => Node }>
  nextBoundaryId: number
  /** Element counter for solid-style hydration keys (per-framework flavor). */
  nextElementId: number
  /** Deterministic scope id (route-derived) for framework flavors. */
  scope: string
  /** Client handler bindings collected during the walk (hydration contract). */
  handlers: Array<HandlerBinding>
}

/** One client handler the server marked with data-ox-on / data-ox-id. */
export interface HandlerBinding {
  /** The deterministic marker id (`h0`, `h1`, …) emitted as data-ox-id. */
  id: string
  /** DOM event name (lowercased `on` prop, e.g. `click`). */
  event: string
  /** The client handler; closes over the component's client hooks. */
  handler: (ev: unknown) => void
}

const FLAVOR_SKIP = new Set(['html', 'head', 'title', 'meta', 'link', 'base', 'body'])

/**
 * Framework flavor, applied per element (excluding the document skeleton).
 * vue: every component element carries the scoped-CSS attribute
 * `data-v-<scope>`; the scope derives deterministically from the route
 * (hash6, src/ir.ts) so markup is byte-deterministic (§7.2).
 */
function flavor(tag: string, attrs: Record<string, string>, state: RenderState): Record<string, string> {
  void tag
  return { ...attrs, [`data-v-${state.scope}`]: '' }
}

// ---------------------------------------------------------------------------
// IR → HTML
// ---------------------------------------------------------------------------

const VOID_TAGS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'])

function isHandlerProp(key: string, value: unknown): value is (ev: unknown) => void {
  return /^on[A-Z]/.test(key) && typeof value === 'function'
}

function eventOf(key: string): string {
  return key.slice(2).toLowerCase()
}

/** Render one element's static attributes (handlers become protocol markers). */
function renderAttrs(node: Extract<Node, { kind: 'element' }>, state: RenderState, flavorAttrs: boolean): string {
  const attrs: Record<string, string> = {}
  for (const [key, value] of Object.entries(node.props)) {
    if (key === 'key' || key === 'ref') continue
    if (isHandlerProp(key, value)) {
      // Client event intent (§7 hydration contract): serialize as protocol
      // markers; hydration re-binds via the deterministic data-ox-id path.
      const id = `h${state.handlers.length}`
      state.handlers.push({ id, event: eventOf(key), handler: value })
      attrs['data-ox-on'] = eventOf(key)
      attrs['data-ox-id'] = id
      continue
    }
    if (value === true) attrs[key] = ''
    else if (value === false || value === undefined || value === null) continue
    else attrs[key] = String(value)
  }
  const final = flavorAttrs ? flavor(node.tag as string, attrs, state) : attrs
  return Object.entries(final)
    .map(([k, v]) => (v === '' ? ` ${k}` : ` ${k}="${escapeAttr(v)}"`))
    .join('')
}

interface WalkResult {
  html: string
  /** Head elements extracted from `el('head', ...)` wrappers (§7 head extraction). */
  head: Node[]
}

/**
 * Walk the IR into HTML. `<head>` wrappers are extracted, never rendered into
 * the body. Component nodes are called recursively (depth-capped).
 */
function walk(node: Node, state: RenderState, depth: number, flavorAttrs: boolean): WalkResult {
  if (depth > 64) throw new Error('render: component tree deeper than 64 (infinite recursion?)')
  switch (node.kind) {
    case 'fragment': {
      const children = node.children.map((c) => walk(c, state, depth + 1, flavorAttrs))
      return { html: children.map((c) => c.html).join(''), head: children.flatMap((c) => c.head) }
    }
    case 'text':
      return { html: escapeText(node.value), head: [] }
    case 'raw':
      return { html: node.html, head: [] }
    case 'defer': {
      const id = state.nextBoundaryId++
      state.defers.push({ id, value: node.value, render: node.render })
      return { html: boundaryFallback(id, walk(node.fallback, state, depth + 1, flavorAttrs).html), head: [] }
    }
    case 'island': {
      // Islands render in their own scope (§10.3): hydration keys and handler
      // ids restart at 0 so a standalone hydrateIsland() render is
      // byte-identical to the SSR island markup. Boundary ids continue the
      // shell sequence (state.defers is shared).
      const islandState: RenderState = { ...state, nextElementId: 0, handlers: [] }
      const ssr = walk(node.children, islandState, depth + 1, flavorAttrs).html
      return {
        html: islandElement(
          {
            component: node.component,
            chunk: node.chunk,
            strategy: node.strategy,
            propsB64: encodePropsB64(node.props ?? {}),
          },
          ssr,
        ),
        head: [],
      }
    }
    case 'element': {
      // Component node: call it with props + children and render the result.
      if (typeof node.tag === 'function') {
        const component = node.tag as ComponentFn
        const result = component({ ...node.props, children: node.children })
        return walk(result, state, depth + 1, flavorAttrs)
      }
      const tag = node.tag
      if (tag === 'head') {
        // Head extraction (§7): the wrapper's children ARE the head nodes;
        // collect them, render nothing into the body.
        return { html: '', head: node.children }
      }
      const attrsHtml = renderAttrs(node, state, flavorAttrs && !FLAVOR_SKIP.has(tag))
      if (VOID_TAGS.has(tag)) return { html: `<${tag}${attrsHtml}>`, head: [] }
      const children = node.children.map((c) => walk(c, state, depth + 1, flavorAttrs))
      return {
        html: `<${tag}${attrsHtml}>${children.map((c) => c.html).join('')}</${tag}>`,
        head: children.flatMap((c) => c.head),
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Head model (packages/oxide types.HeadTags)
// ---------------------------------------------------------------------------

const metaKey = (m: Record<string, string>): string => m.name ?? m.property ?? m.charset ?? m.httpEquiv ?? ''

/**
 * Merge two neutral head models — a tiny replica of render::HeadTags::merge:
 * the inner (page/route) title wins but keeps the outer (layout) position;
 * titleTemplate comes from the layout; other lists append in inner order with
 * meta entries overriding on their identifying key (name/property/…).
 */
export function mergeHead(outer: HeadTags, inner: Partial<HeadTags>): HeadTags {
  const meta: Array<Record<string, string>> = outer.meta.map((m) => ({ ...m }))
  const seen = new Set(meta.map(metaKey).filter(Boolean))
  for (const m of inner.meta ?? []) {
    const key = metaKey(m)
    const existing = key ? meta.findIndex((x) => metaKey(x) === key) : -1
    if (existing >= 0) meta[existing] = { ...meta[existing], ...m }
    else meta.push({ ...m })
    if (key) seen.add(key)
  }
  return {
    title: inner.title ?? outer.title,
    // A layout title that contains %s IS the template (§16.3): when the page
    // sets a concrete title, carry the layout's templated title over.
    titleTemplate: inner.titleTemplate ?? (outer.title?.includes('%s') ? outer.title : outer.titleTemplate),
    base: inner.base ?? outer.base,
    meta,
    link: [...outer.link, ...(inner.link ?? [])].map((l) => ({ ...l })),
    style: [...outer.style, ...(inner.style ?? [])].map((s) => ({ ...s })),
    script: [...outer.script, ...(inner.script ?? [])].map((s) => ({ ...s })),
    jsonLd: [...(outer.jsonLd ?? []), ...(inner.jsonLd ?? [])],
  }
}

/** Apply a `%s` title template ("Head Test" + "%s · Atlas" → "Head Test · Atlas"). */
export function applyTitleTemplate(head: HeadTags): string | undefined {
  if (!head.title) return undefined
  if (head.titleTemplate && head.titleTemplate.includes('%s')) {
    return head.titleTemplate.replace('%s', head.title)
  }
  return head.titleTemplate ?? head.title
}

function renderHeadHtml(head: HeadTags): string {
  const parts: string[] = []
  const title = applyTitleTemplate(head)
  if (title !== undefined) parts.push(`<title>${escapeText(title)}</title>`)
  if (head.base) parts.push(`<base href="${escapeAttr(head.base.href)}"${head.base.target ? ` target="${escapeAttr(head.base.target)}"` : ''}>`)
  for (const m of head.meta) {
    const attrs = Object.entries(m).map(([k, v]) => ` ${k}="${escapeAttr(v)}"`).join('')
    parts.push(`<meta${attrs}>`)
  }
  for (const l of head.link) {
    const attrs = Object.entries(l).map(([k, v]) => ` ${k}="${escapeAttr(v)}"`).join('')
    parts.push(`<link${attrs}>`)
  }
  for (const s of head.style) parts.push(`<style${s.nonce ? ` nonce="${escapeAttr(s.nonce)}"` : ''}>${s.content}</style>`)
  for (const s of head.script) {
    const attrs = [
      s.src ? ` src="${escapeAttr(s.src)}"` : '',
      s.type ? ` type="${escapeAttr(s.type)}"` : '',
      s.nonce ? ` nonce="${escapeAttr(s.nonce)}"` : '',
      s.defer ? ' defer' : '',
    ].join('')
    parts.push(`<script${attrs}>${s.content ? escapeScript(s.content) : ''}</script>`)
  }
  for (const json of head.jsonLd ?? []) {
    parts.push(`<script type="application/ld+json">${escapeScript(JSON.stringify(json))}</script>`)
  }
  return parts.join('')
}

// ---------------------------------------------------------------------------
// The Atlas page tree (§22 reference app; identical routes per adapter)
// ---------------------------------------------------------------------------

interface PageProps {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: Record<string, unknown>
  /** Client-only context — undefined during SSR, wired by src/hydrate.ts. */
  client?: ClientHooks
}

/** Client hooks the hydration layer injects (§7 hydrate/update contract). */
export interface ClientHooks {
  /** Re-render the current route tree with patched data (adapter.update). */
  update(patch: Record<string, unknown>): void
  /** Client navigation hook (the core router intercepts links first). */
  navigate(href: string): void
}

interface AtlasPage {
  component: ComponentFn
}

function HomePage(props: PageProps): Node {
  return el('section', { class: 'home' },
    el('h1', {}, t('Hello Atlas')),
    el('p', { class: 'intro' }, t(`Serving ${String(props.data.framework ?? 'every framework')} from one core.`)),
    el('a', { href: '/blog', class: 'link', onClick: (ev: unknown) => { props.client?.navigate('/blog'); void ev } }, t('Read the blog')),
    // Head extraction (§7): components contribute head elements via <head>.
    el('head', {}, el('meta', { name: 'ox-page', content: 'atlas-home' })),
  )
}

interface PageProps {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: Record<string, unknown>
  /** Client-only hooks — undefined during SSR, wired by src/hydrate.ts. */
  client?: { update(patch: Record<string, unknown>): void; navigate(href: string): void }
}

function BlogPage(props: PageProps): Node {
  const comments = props.data.comments as Array<{ author: string; body: string }> | undefined
  return el('article', { class: 'post' },
    el('h1', {}, t(String(props.data.title ?? 'Untitled'))),
    el('p', { class: 'byline' }, t(`By ${String(props.data.author ?? 'Anonymous')} · ${String(props.params.slug ?? '')}`)),
    el('div', { class: 'body' }, t(String(props.data.body ?? ''))),
    comments
      ? el('ul', { class: 'comments' },
          comments.map((c) => el('li', { class: 'comment' }, el('strong', {}, t(c.author)), t(` ${c.body}`))))
      : null,
    el('head', {}, el('meta', { name: 'author', content: String(props.data.author ?? 'Atlas') })),
  )
}

function StreamingPage(props: PageProps): Node {
  return el('section', { class: 'stream' },
    el('h1', {}, t('Streaming Atlas')),
    deferred(
      props.data.comments as DeferSource,
      el('p', { class: 'fallback' }, t('Loading comments…')),
      (resolved) => {
        const comments = (resolved ?? []) as Array<{ author: string; body: string }>
        return el('ul', { class: 'comments' },
          comments.map((c) => el('li', { class: 'comment' }, el('strong', {}, t(c.author)), t(` ${c.body}`))))
      },
    ),
    // §7.1 out-of-order streaming fixture: a SECOND deferred boundary,
    // declared after `comments` but resolving sooner. A sequential renderer
    // yields comments-then-meta; the concurrent renderer yields meta first.
    deferred(
      props.data.meta as DeferSource,
      el('p', { class: 'fallback' }, t('Loading meta…')),
      (resolved) => el('span', { class: 'meta' }, t(String(resolved ?? ''))),
    ),
  )
}

function ErrorPage(): Node {
  // §10.2 step 5: a render error maps to the core error boundary.
  throw new Error('Atlas render failure: the data module exploded')
}

function NotFoundPage(): Node {
  return el('section', { class: 'notfound', 'data-ox-not-found': '' },
    el('h1', {}, t('404')),
    el('p', {}, t('This Atlas route does not exist.')),
  )
}

function CounterIsland(props: PageProps & { initialCount?: number }): Node {
  return el('div', { class: 'counter' },
    el('span', { class: 'count' }, t(String(props.initialCount ?? 0))),
    el('button', {
      class: 'inc',
      onClick: () => props.client?.update({ initialCount: Number(props.initialCount ?? 0) + 1 }),
    }, t('Increment')),
  )
}

function IslandsPage(props: PageProps): Node {
  return el('section', { class: 'islands' },
    el('h1', {}, t('Islands Atlas')),
    island(
      {
        component: 'Counter',
        chunk: '/islands/counter.ts',
        strategy: 'load',
        props: { initialCount: Number(props.data.count ?? 0) },
      },
      el(CounterIsland, { initialCount: Number(props.data.count ?? 0) }),
    ),
  )
}

function ActionsPage(props: PageProps): Node {
  const form = props.data.form as { name?: string } | undefined
  return el('section', { class: 'actions' },
    el('h1', {}, t('Actions Atlas')),
    el('form', { method: 'POST', action: '/actions', class: 'form' },
      el('input', { name: 'name', value: form?.name ?? '', class: 'input' }),
      el('button', { type: 'submit', class: 'submit' }, t('Say hello')),
    ),
    form?.name
      ? el('p', { class: 'form-result' }, t(`Hello ${form.name}`))
      : raw('<!--form-result-pending-->'),
  )
}

function CachedPage(props: PageProps): Node {
  const posts = (props.data.posts ?? []) as string[]
  return el('section', { class: 'cached' },
    el('h1', {}, t('Cache Atlas')),
    el('ul', { class: 'posts' }, posts.map((p) => el('li', { class: 'post-item' }, t(p)))),
  )
}

function HeadPage(): Node {
  return el('section', { class: 'head-check' },
    el('p', {}, t('Head check')),
    el('head', {}, el('meta', { name: 'ox-page', content: 'page-head-extracted' })),
  )
}

/** The build-generated route → page registry (spec §8.1 page conventions). */
const PAGES: Record<string, AtlasPage> = {
  'routes/index': { component: HomePage },
  'routes/blog/[slug]': { component: BlogPage },
  'routes/streaming': { component: StreamingPage },
  'routes/error': { component: ErrorPage },
  'routes/not-found': { component: NotFoundPage },
  'routes/islands': { component: IslandsPage },
  'routes/actions': { component: ActionsPage },
  'routes/cached': { component: CachedPage },
  'routes/head': { component: HeadPage },
}

/** Look up a registered page component (client hydration reuses the registry). */
export function pageComponent(route: string): ComponentFn {
  return (PAGES[route] ?? { component: NotFoundPage }).component
}

/** Test/build helper: the island chunk map (§10.3 "each island is its own chunk"). */
export const islandModules: Record<string, { default: ComponentFn }> = {
  '/islands/counter.ts': { default: CounterIsland as ComponentFn },
}

/** Test/build helper: pages that throw at render (error-boundary conformance). */
export const throwsAtRender = new Set(['routes/error'])

/**
 * The root layout (spec §8.1 layout.*): a fragment of layout head + the page
 * — the SHELL owns the document skeleton (`<!doctype html><html><head>` +
 * `<body>`) and the framework root container below.
 */
function AtlasLayout(props: PageProps & { page: Node }): Node {
  return frag(
    el('head', {},
      // Layout title doubles as the title template (§16.3 merge): "%s · Atlas".
      el('title', {}, t('%s · Atlas')),
      el('meta', { charset: 'utf-8' }),
      el('meta', { name: 'viewport', content: 'width=device-width, initial-scale=1' }),
    ),
    props.page,
  )
}

// ---------------------------------------------------------------------------
// Shell + streaming render
// ---------------------------------------------------------------------------

const LAYOUT_HEAD: HeadTags = {
  title: undefined,
  titleTemplate: '%s · Atlas',
  meta: [{ charset: 'utf-8' }, { name: 'viewport', content: 'width=device-width, initial-scale=1' }],
  link: [],
  style: [],
  script: [],
}

interface Shell {
  headHtml: string
  bodyHtml: string
  defers: Array<{ id: number; value: DeferSource; render: (resolved: unknown) => Node }>
}

function buildTree(input: RenderInput): Node {
  const page = PAGES[input.route] ?? { component: NotFoundPage }
  const pageProps = {
    route: input.route,
    url: input.url,
    params: input.params,
    data: input.data as Record<string, unknown>,
  }
  const pageNode = el(page.component, pageProps)

  // §8.2 / §16.3 layout nesting: this IR renderer is the **conformance**
  // renderer for the Atlas reference app. Its `PAGES` registry holds page
  // components only — there are no layout components to resolve a chain
  // against, so `input.layouts` is deliberately ignored here and the page is
  // always wrapped in the default `AtlasLayout` shell. That shell is the
  // markup contract the real adapter (`real-adapter.ts` / `makeRealAdapter`)
  // must also emit for a route with no layouts, and the real adapter's
  // multi-level chain is asserted separately in
  // `packages/oxide/test/nested-layouts.test.ts`. Resolving `input.layouts`
  // here would risk a page id accidentally swallowing the page's children
  // (a page component ignores `props.children`), so it is intentionally a
  // no-op — the IR's job is the *shell* contract, not user component wiring.
  return el(AtlasLayout, { ...pageProps, page: pageNode })
}

/** Convert head elements extracted from the page tree into the neutral model.
 * A layout title containing `%s` IS the title template (§16.3 convention). */
function headNodesToTags(nodes: Node[]): Partial<HeadTags> {
  const tags: Partial<HeadTags> = { meta: [], link: [], style: [], script: [] }
  for (const node of nodes) {
    if (node.kind !== 'element' || typeof node.tag !== 'string') continue
    if (node.tag === 'title') {
      const title = node.children.map((c) => (c.kind === 'text' ? c.value : '')).join('')
      if (title.includes('%s')) tags.titleTemplate = title
      else tags.title = title
    }
    else if (node.tag === 'meta') tags.meta!.push({ ...node.props } as Record<string, string>)
    else if (node.tag === 'link') tags.link!.push({ ...node.props } as Record<string, string>)
    else if (node.tag === 'style') tags.style!.push({ content: node.children.map((c) => (c.kind === 'text' ? c.value : '')).join('') })
    else if (node.tag === 'base') tags.base = { href: String(node.props.href ?? '/') }
  }
  return tags
}

/**
 * The framework root container: Vue mounts into it, and createApp marks the
 * container with `data-v-app` (per-framework flavor lives here).
 */
const ROOT_ATTRS: Record<string, string> = { id: 'root', 'data-v-app': '' }

/**
 * Build the shell: merged head (layout ⊕ loader ⊕ extracted page head) + the
 * document body with `template[data-ox-b]` placeholders for every deferred
 * boundary. Merge order mirrors render::HeadTags::merge: outer = layout,
 * then the loader/route head (RenderInput.head), then page-extracted head
 * (inner-most wins on conflicts, appended in inner order).
 */
function buildShell(input: RenderInput, state: RenderState): Shell {
  const rootDiv = el('div', { ...ROOT_ATTRS }, buildTree(input))
  const walked = walk(rootDiv, state, 1, true)
  const merged = mergeHead(mergeHead(LAYOUT_HEAD, input.head), headNodesToTags(walked.head))
  const dataScript = input.odf ? hydrationBootstrap(Buffer.from(input.odf).toString('base64'), input.route) : ''
  const headHtml = `<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${renderHeadHtml(merged)}</head>`
  const bodyHtml = `<body>${walked.html}${dataScript}</body></html>`
  return { headHtml, bodyHtml, defers: state.defers }
}

/** Error shell (§10.2 step 5): the core error boundary replaces the tree. */
function errorShell(input: RenderInput, err: unknown): Shell {
  const message = err instanceof Error ? err.message : String(err)
  // §10.2 step 5: prefer the project's `error.tsx` component when one
  // resolves through this adapter's registry. The real adapter passes an
  // `input.errorRoute` the same way; the IR keeps the same shape so the
  // conformance suite can assert it.
  const errorComponent = input.errorRoute ? PAGES[input.errorRoute] : undefined
  if (errorComponent) {
    const state = newRenderState(`${input.route}#error`)
    try {
      const tree = el(errorComponent.component, {
        route: input.route,
        url: input.url,
        params: input.params,
        data: input.data as Record<string, unknown>,
        error: { message, route: input.route, status: 500 },
      })
      const walked = walk(tree, state, 1, true)
      const head = mergeHead(LAYOUT_HEAD, input.head)
      return {
        headHtml: `<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${renderHeadHtml(head)}</head>`,
        bodyHtml: `<body>${walked.html}</body></html>`,
        defers: state.defers,
      }
    } catch {
      // The error page itself threw — fall through to the generic boundary.
    }
  }
  const head = mergeHead(LAYOUT_HEAD, { title: 'Error', meta: [], link: [], style: [], script: [] })
  return {
    headHtml: `<!doctype html>\n<html lang="${input.locale && /^[A-Za-z0-9_-]+$/.test(input.locale) ? input.locale : 'en'}"><head>${renderHeadHtml(head)}</head>`,
    bodyHtml: `<body>${errorBoundary('OX-500', message)}</body></html>`,
    defers: [],
  }
}

export function newRenderState(route: string): RenderState {
  return { defers: [], nextBoundaryId: 0, nextElementId: 0, scope: hash6(route), handlers: [] }
}

/** Render one standalone IR node (island hydration uses this path). */
export function renderNode(node: Node, state: RenderState): { html: string; head: Node[] } {
  return walk(node, state, 1, true)
}

/** Render one standalone IR node to a string with a fresh route-scoped state. */
export function renderNodeToString(node: Node, state: RenderState): string {
  return renderNode(node, state).html
}

/**
 * Rebuild the deterministic handler bindings for a tree: render it into a
 * fresh state and return the collected data-ox-on / data-ox-id bindings in
 * marker order. The client binds these onto the live DOM (src/hydrate.ts).
 */
export function walkTreeForHandlers(tree: Node): HandlerBinding[] {
  const state = newRenderState('')
  walk(tree, state, 1, true)
  return state.handlers
}

/**
 * Streaming render (§7 `render(input) → ReadableStream<Uint8Array>`).
 * Pull-driven (backpressure): the next chunk is produced only on pull.
 * Chunk plan: [head] [body shell] then one chunk per deferred resolution
 * (`template[data-ox-c]` + swapper, §7.1). A rejecting boundary streams the
 * error boundary content instead (§10.2 step 5).
 */
export function render(input: RenderInput): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder()
  let state: RenderState
  let shell: Shell
  try {
    state = newRenderState(input.route)
    shell = buildShell(input, state)
  } catch (err) {
    state = newRenderState(input.route)
    shell = errorShell(input, err)
  }
  const it = (async function* (): AsyncGenerator<Uint8Array> {
    yield encoder.encode(shell.headHtml)
    yield encoder.encode(shell.bodyHtml)
    // §7.1 out-of-order streaming: kick every defer off concurrently and
    // yield each boundary as soon as IT settles — not in declaration order.
    // A slow boundary no longer delays a fast one, which is the whole point
    // of the deferred-boundary protocol. The swapper matches by id, so the
    // resolution chunks are order-independent on the client side.
    const resolved: Array<{ id: number; html: string }> = []
    let wakeup: (() => void) | undefined
    const wake = (): void => {
      const w = wakeup
      wakeup = undefined
      w?.()
    }
    for (const d of shell.defers) {
      void (async () => {
        let html: string
        try {
          // Fresh state per resolution: resolution markup is deterministic
          // and independent of the shell walk (hydration re-binds swapped
          // content after the ox:swap event, §7.1).
          html = walk(d.render(await deferPromise(d.value)), newRenderState(input.route), 1, true).html
        } catch (err) {
          html = errorBoundary('OX-500', err instanceof Error ? err.message : String(err))
        }
        resolved.push({ id: d.id, html })
        wake()
      })()
    }
    const total = shell.defers.length
    for (let i = 0; i < total; i++) {
      while (resolved.length === 0) {
        await new Promise<void>((r) => {
          wakeup = r
        })
      }
      const r = resolved.shift()!
      yield encoder.encode(boundaryContent(r.id, r.html))
    }
  })()
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const { value, done } = await it.next()
        if (done) controller.close()
        else controller.enqueue(value)
      } catch (err) {
        controller.error(err)
      }
    },
  })
}

/** Drain the stream into one HTML string (shell + every resolution). */
export async function renderToString(input: RenderInput): Promise<string> {
  const parts: string[] = []
  for await (const chunk of render(input)) parts.push(new TextDecoder().decode(chunk))
  return parts.join('')
}

/**
 * Shell-only render (sync): placeholders stay unresolved. The client uses
 * this for update()/mount() — streamed slots are owned by the server stream
 * and the ox:swap glue, not by client re-renders.
 */
export function renderShellToString(input: RenderInput): string {
  const state = newRenderState(input.route)
  let shell: Shell
  try {
    shell = buildShell(input, state)
  } catch (err) {
    shell = errorShell(input, err)
  }
  return shell.headHtml + shell.bodyHtml
}

// ---------------------------------------------------------------------------
// head(ctx) — framework head model → neutral model (§7)
// ---------------------------------------------------------------------------

/**
 * `head(ctx)`: builds the React-flavored head model from the render context
 * and merges it into the neutral HeadTags model (packages/oxide types.ts).
 * Route-level head tags may be passed through `ctx.locals['oxide:head']`
 * (the pipeline registers loader meta there); they override the defaults,
 * and the layout title template applies at emit time.
 */
export function head(ctx: RenderCtx): HeadTags {
  const data = (ctx.data ?? {}) as Record<string, unknown>
  const base: HeadTags = {
    title: typeof data.title === 'string' ? data.title : undefined,
    titleTemplate: '%s · Atlas',
    meta: [
      { charset: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      ...(typeof data.description === 'string' ? [{ name: 'description', content: data.description }] : []),
    ],
    link: [],
    style: [],
    script: [],
  }
  const routeHead = (ctx.locals?.['oxide:head'] ?? {}) as Partial<HeadTags>
  return mergeHead(base, routeHead)
}
