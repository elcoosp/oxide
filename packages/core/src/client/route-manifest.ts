// Manifest-driven route → component resolution on the client (spec §6.4 + §8.4).
//
// The bundler emits `client.manifest.json` (`crates/bundler/src/lib.rs`
// ClientManifest: `{ buildId, routes: { "<route id>": { entry_chunk, preload,
// css, js_bytes, modules } }, assets }`). The generated boot entry fetches it
// and hands it to `bootWithRouter({ manifest })`, which registers it here
// (`setRouteComponentSource`). Every framework adapter's client `update()` /
// `hydrate()` / `mount()` then resolves the route's REAL page component
// through {@link componentFor} instead of replaying the deterministic IR
// Atlas registry (§10.3 client half of the §7 adapter contract).
//
// Design for testability: the chunk importer is part of the injectable
// {@link RouteComponentSource} — tests register a stub manifest + a stub
// `loadChunk` and drive the whole resolution without a bundler or a browser.
// No dependencies: matching is segment arithmetic over the manifest keys.

/**
 * One route's client bundle, exactly the `RouteBundle` shape the bundler
 * serializes into `client.manifest.json` (§6.4). `entry_chunk` is a path
 * relative to the client output dir (served under `/client/…`).
 */
export interface RouteManifestEntry {
  /** The route's entry JS chunk (preload order §10.2 step 4). */
  entry_chunk?: string
  /** Extra chunks to preload, entry first. */
  preload?: string[]
  /** Legacy/alias field for chunk lists (kept for older manifests). */
  chunks?: string[]
  css?: string[]
  server?: string
  renderMode?: string
  js_bytes?: number
  modules?: string[]
  /**
   * §8.2 / §16.3 / O1: layout chain, outermost → innermost. Each entry
   * is a manifest route id (routes/layout, routes/nested/layout) — not
   * a page key with a trailing slash-page segment. Populated by the
   * build's enrichment pass; older manifests have no field and parse as
   * the empty chain.
   */
  layout_chain?: string[]
}

/** The subset of the wire manifest the client runtime consumes (§6.4). */
export interface ClientManifestLike {
  buildId?: string
  routes: Record<string, RouteManifestEntry>
}

/**
 * The injectable source the client runtime resolves page components from.
 * `loadChunk` is the boot runtime's chunk loader (`window.__oxideBoot
 * .loadChunk`) by default; tests stub it. The importer receives a
 * root-relative URL (e.g. `/client/page-abc123.js`).
 */
export interface RouteComponentSource {
  manifest: ClientManifestLike
  loadChunk: (chunk: string) => Promise<unknown>
}

let source: RouteComponentSource | undefined

/**
 * Register (or clear, with `undefined`) the manifest + chunk loader the
 * adapters resolve page components from. Called by `bootWithRouter` when the
 * boot entry hands it a manifest; idempotent per boot.
 */
export function setRouteComponentSource(next: RouteComponentSource | undefined): void {
  source = next
}

/** The currently registered source, or `undefined` (IR fallback territory). */
export function getRouteComponentSource(): RouteComponentSource | undefined {
  return source
}

/**
 * True when a path segment is a dynamic-route parameter token. Covers every
 * spelling the build emits: `[name]` and `[...rest]` (the scanner keeps the
 * bracket dir token verbatim), `_name_` (the manifest's normalized form), and
 * `:name` (hand-written route tables). Shared by the {@link routeIdFor}
 * matcher and boot.ts's loading-map matcher (§8.4).
 */
export function isParamSegment(seg: string): boolean {
  if (seg.startsWith(':')) return seg.length > 1
  if (seg.startsWith('[') && seg.endsWith(']')) return seg.length > 2
  // The manifest historically normalizes `[slug]` → `_slug_`.
  return seg.length > 2 && seg.startsWith('_') && seg.endsWith('_')
}

/** Split a URL path or manifest-key body into non-empty segments. */
function segmentsOf(path: string): string[] {
  return path.split('/').filter((s) => s.length > 0)
}

/**
 * Does a manifest key's segment list match a URL's segment list?
 *
 * Matching rules (§8.4, the same contract the CLI's loading-map keys use):
 *   - a literal key segment equals the URL segment;
 *   - a param key segment (`[name]`, `[...name]`, `_name_`, `:name`) matches
 *     exactly ONE non-empty URL segment;
 *   - `routes/index/page` is the manifest key for the root URL `/`
 *     (normalized to the literal segment `index` before comparison, so it
 *     only matches the root, not `/index`);
 *   - the key may be a PREFIX of the URL (a layout-level boundary covers the
 *     routes below it) — {@link routeIdFor} only uses full matches, the
 *     loading-map matcher in `boot.ts` uses prefix matches.
 *
 * Returns the match quality (static-segment count) when the key matches
 * `urlSegs` in full, or `undefined` when it doesn't.
 */
function matchKeySegments(keySegs: string[], urlSegs: string[]): number | undefined {
  if (keySegs.length !== urlSegs.length) return undefined
  let staticCount = 0
  for (let i = 0; i < keySegs.length; i++) {
    const k = keySegs[i]
    const u = urlSegs[i]
    if (k === undefined || u === undefined) return undefined
    if (isParamSegment(k)) {
      if (u.length === 0) return undefined
      continue
    }
    if (k !== u) return undefined
    staticCount++
  }
  return staticCount
}

/**
 * Resolve the manifest route id for a route reference. Accepts every shape
 * the runtime hands adapters:
 *   - a full manifest key: `routes/blog/[slug]/page`;
 *   - a route label (what `/__data` and `window.__oxide` carry):
 *     `routes/blog/[slug]` — the scanner id without the `/page` suffix;
 *   - a URL path: `/blog/hello`, `/` (segment-matched against the keys).
 *
 * Returns the exact manifest key, or `undefined` when the manifest has no
 * route for it (callers fall back to the IR renderer). Ties between a param
 * and a literal segment at the same depth resolve to the literal (higher
 * static-segment count), mirroring the router's own matcher.
 */
export function routeIdFor(routeRef: unknown, manifest: ClientManifestLike): string | undefined {
  if (!routeRef || typeof routeRef !== 'string') return undefined
  // Strip query/hash for URL-shaped references.
  const ref = routeRef.split(/[?#]/, 1)[0] ?? routeRef
  const routes = manifest?.routes ?? {}
  // Only `routes/<…>/page` rows (plus the root's `routes/index/page`) are
  // client page chunks — endpoint rows and `__boot__` are not importable
  // page components even when a caller hands their id in verbatim.
  const isPageKey = (k: string): boolean => k === 'routes/index/page' || /^routes\/.+\/page$/.test(k)
  // (1) Exact key.
  if (routes[ref] !== undefined && isPageKey(ref)) return ref
  // (2) Label form: manifest key minus `/page`.
  if (!ref.endsWith('/page') && isPageKey(`${ref}/page`) && routes[`${ref}/page`] !== undefined) {
    return `${ref}/page`
  }
  // (3) URL path segment-match against `routes/<…>/page` keys.
  let urlSegs = segmentsOf(ref.replace(/^\/+/, ''))
  // The root URL `/` (and the bare `routes/index` label) map to the
  // scanner's root page id `routes/index/page`.
  if (ref === '/' || ref === '' || ref === 'routes/index' || ref === 'index') urlSegs = []
  let best: string | undefined
  let bestStatic = -1
  for (const key of Object.keys(routes)) {
    if (key === '__boot__') continue
    const m = /^routes\/(.*?)\/page$/.exec(key)
    if (!m) continue
    const keySegs = m[1] ? segmentsOf(m[1]) : []
    // Root: `routes/index/page` → segments `['index']`, URL `/` → `[]`.
    const effectiveKey = keySegs.length === 1 && keySegs[0] === 'index' ? [] : keySegs
    const staticCount = matchKeySegments(effectiveKey, urlSegs)
    if (staticCount === undefined) continue
    if (staticCount > bestStatic) {
      bestStatic = staticCount
      best = key
    }
  }
  return best
}

/**
 * §8.2 / O1: the layout chunks a route needs, given a matched manifest key.
 *
 * Each page's manifest entry carries `layout_chain` — the layout route ids
 * (e.g. `routes/layout`), which the bundler emits directly as RouteBundles
 * with no page suffix. This walks the chain and returns the layout chunk
 * names (entry + preload) so a preload walk can warm them in parallel with
 * the page.
 *
 * Pure and dependency-free: the caller has already resolved the page's
 * manifest key. Returns an empty array when the route has no entry, no
 * `layout_chain`, or a chain referencing ids the manifest does not carry —
 * a missing layout chunk is simply not preloaded, never an error.
 */
export function layoutChunksFor(
  routeId: string,
  manifest: ClientManifestLike,
): string[] {
  const entry = manifest?.routes?.[routeId]
  if (!entry?.layout_chain || entry.layout_chain.length === 0) return []
  const out: string[] = []
  for (const layoutId of entry.layout_chain) {
    const layout = manifest.routes[layoutId]
    if (!layout) continue
    if (layout.entry_chunk) out.push(layout.entry_chunk)
    if (layout.preload) out.push(...layout.preload)
  }
  return out
}

/**
 * Extract the page component export from a dynamically imported route chunk
 * module: `default` first, then the conventional named exports, then the
 * module itself when it is a plain component function. Returns `undefined`
 * when nothing component-shaped is found (callers fall back to IR).
 */
export function pageComponentExport(mod: unknown): unknown {
  if (mod === undefined || mod === null) return undefined
  const m = mod as Record<string, unknown>
  const candidates = [m.default, m.Page, m.page, m.Component, m.component]
  for (const c of candidates) {
    if (c !== undefined && c !== null) return c
  }
  if (typeof mod === 'function') return mod
  return undefined
}

/** Turn a manifest chunk path into a root-relative URL the importer can fetch. */
function chunkUrl(chunk: string): string {
  if (/^(https?:)?\/\//.test(chunk) || chunk.startsWith('/')) return chunk
  // Bundler chunk paths are relative to the client output dir (§6.4).
  return `/client/${chunk}`
}

/**
 * Manifest-driven `componentFor(routeId)` (§8.4): resolve the route's
 * manifest entry, dynamically import its client entry chunk through the
 * registered {@link RouteComponentSource.loadChunk}, and return the page
 * component export. Returns `undefined` (never throws) when no source is
 * registered, the route isn't in the manifest, the chunk fails to load, or
 * the module carries no component export — every caller must treat that as
 * "fall back to the IR renderer".
 */
export async function componentFor(routeRef: string): Promise<unknown> {
  const src = source
  if (!src) return undefined
  // §8.2: layouts (`routes/layout`, no `/page` suffix) live in the manifest
  // under their direct id. `routeIdFor` intentionally only normalizes PAGE
  // ids (`routes/foo/page` / `routes/index` → `routes/foo/page`), so it
  // rejects a bare `routes/layout` even though the bundler now emits that
  // chunk. A direct-key hit takes precedence and makes layouts hydratable —
  // which lets the client rebuild the server's `<RootLayout><Page/></RootLayout>`
  // tree instead of wiping the SSR markup with a fresh `createRoot` mount.
  const directKey =
    src.manifest.routes[routeRef] !== undefined ? routeRef : undefined
  const key = directKey ?? routeIdFor(routeRef, src.manifest)
  if (key === undefined) return undefined
  const entry = src.manifest.routes[key]
  if (!entry) return undefined
  const chunk = entry.entry_chunk ?? entry.preload?.[0] ?? entry.chunks?.[0]
  if (!chunk) return undefined
  try {
    const mod = await src.loadChunk(chunkUrl(chunk))
    return pageComponentExport(mod)
  } catch {
    // A missing/broken chunk is not fatal — the IR fallback still paints.
    return undefined
  }
}
