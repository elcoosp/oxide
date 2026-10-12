// Framework adapter contract types (spec §7) — the cross-crate/cross-package
// interface every framework adapter package (@oxide/react, @oxide/vue, …)
// implements. Frozen before first public beta; versioned via `contract: 1`.

/** Rollup/Vite-shaped plugin object (§15.2). Runtime-agnostic on purpose. */
export type Plugin = {
  name: string
  enforce?: 'pre' | 'post'
  [hook: string]: unknown
}

/** Neutral head model the core merges across layouts (§16.3). */
export interface HeadTags {
  title?: string
  titleTemplate?: string
  base?: { href: string; target?: string }
  meta: Array<Record<string, string>> // name/property/charset/httpEquiv + content
  link: Array<Record<string, string>>
  style: Array<{ content: string; nonce?: string }>
  script: Array<{ content?: string; src?: string; type?: string; nonce?: string; defer?: boolean }>
  jsonLd?: unknown[]
}

/** Input to adapter.render() — layouts + page + data (§7). */
export interface RenderInput {
  /** Matched route id, e.g. `routes/blog/[slug]`. */
  route: string
  url: string
  params: Record<string, string | string[]>
  /** Loader data (ODF-decoded); deferred slots arrive as promise-likes. */
  data: unknown
  head: HeadTags
  /** Serialized ODF payload for `<script type='application/oxide-data'>`. */
  odf?: Uint8Array
  locals?: Record<string, unknown>
}

/** Context for adapter.head() — framework head model → neutral model. */
export interface RenderCtx {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: unknown
  locals?: Record<string, unknown>
}

/** Input to adapter.update()/hydrate()/mount() on the client (§7). */
export interface ClientInput {
  route: string
  url: string
  params: Record<string, string | string[]>
  data: unknown
  /** Previous navigation input, when diffing (optional). */
  from?: ClientInput
  /** Search params of the navigation. */
  searchParams?: URLSearchParams
}

export type Disposer = () => void

/**
 * The framework adapter contract (spec §7). The adapter owns components,
 * rendering, hydration and HMR; the core owns routes, loaders, actions, cache,
 * head merging, serialization, middleware, client navigation and streaming.
 */
export interface FrameworkAdapter {
  contract: 1
  name: string
  /** e.g. ['.tsx'] | ['.vue'] | ['.svelte'] */
  extensions: string[]
  jsxImportSource?: string
  capabilities: {
    /** Who owns streaming boundaries. */
    suspense: 'native' | 'core'
    islands: boolean
    hmr: 'refresh' | 'component' | 'full'
    /** Reserved (post-MLP, D17). */
    rsc?: boolean
  }
  /** Framework compile plugins for the tooling pool. */
  plugins(opts: unknown): Plugin[]

  // ---- server entry ----
  render(input: RenderInput): ReadableStream<Uint8Array>
  head(ctx: RenderCtx): HeadTags

  // ---- client entry ----
  hydrate(root: Element, input: ClientInput): Disposer
  /** Islands mode (§10.3); adapters without islands omit it. */
  hydrateIsland?(el: Element, mod: unknown, props: unknown): Disposer
  /** SPA mode shell mount. */
  mount(root: Element, input: ClientInput): Disposer
  /** Client navigation: swap the route tree. */
  update(input: ClientInput): void
}

/** Client route manifest handed to the client router (from the build, §6.4). */
export interface ClientManifest {
  buildId?: string
  routes: Record<string, ClientRouteEntry>
}

export interface ClientRouteEntry {
  /** Server data module path (page.server.ts), when the route has one. */
  server?: string
  renderMode?: 'static' | 'isr' | 'dynamic' | 'spa' | 'islands'
  /** Client entry chunks in preload order. */
  chunks?: string[]
  css?: string[]
}
