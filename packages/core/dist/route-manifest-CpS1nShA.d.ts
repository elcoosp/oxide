/**
 * One route's client bundle, exactly the `RouteBundle` shape the bundler
 * serializes into `client.manifest.json` (§6.4). `entry_chunk` is a path
 * relative to the client output dir (served under `/client/…`).
 */
interface RouteManifestEntry {
  /** The route's entry JS chunk (preload order §10.2 step 4). */
  entry_chunk?: string;
  /** ESM-dev manifest entry: the source URL of the route's component.
   *  Set by `oxide dev` in `.oxide/esm-manifest.json`; absent in bundler mode.
   *  `componentFor` checks this before `entry_chunk` so a dev manifest resolves correctly. */
  component?: string;
  /** Extra chunks to preload, entry first. */
  preload?: string[];
  /** Legacy/alias field for chunk lists (kept for older manifests). */
  chunks?: string[];
  css?: string[];
  server?: string;
  renderMode?: string;
  js_bytes?: number;
  modules?: string[];
  /**
   * §8.2 / §16.3 / O1: layout chain, outermost → innermost. Each entry
   * is a manifest route id (routes/layout, routes/nested/layout) — not
   * a page key with a trailing slash-page segment. Populated by the
   * build's enrichment pass; older manifests have no field and parse as
   * the empty chain.
   */
  layout_chain?: string[];
}
/** The subset of the wire manifest the client runtime consumes (§6.4). */
interface ClientManifestLike {
  buildId?: string;
  routes: Record<string, RouteManifestEntry>;
}
/**
 * The injectable source the client runtime resolves page components from.
 * `loadChunk` is the boot runtime's chunk loader (`window.__oxideBoot
 * .loadChunk`) by default; tests stub it. The importer receives a
 * root-relative URL (e.g. `/client/page-abc123.js`).
 */
interface RouteComponentSource {
  manifest: ClientManifestLike;
  loadChunk: (chunk: string) => Promise<unknown>;
}
/**
 * Register (or clear, with `undefined`) the manifest + chunk loader the
 * adapters resolve page components from. Called by `bootWithRouter` when the
 * boot entry hands it a manifest; idempotent per boot.
 *
 * State lives on `globalThis`, not a module-local binding, because the client
 * bundler can emit this module more than once in one page: the boot entry and
 * a framework adapter's chunk reach `route-manifest.ts` through different
 * relative specifiers, so Rolldown treats them as two module instances. A
 * module-local binding would be written by the boot copy and read (still
 * `undefined`) by the adapter copy, leaving every navigation on the IR
 * fallback — the client-side analogue of the SSR two-React bug. `globalThis`
 * is per-realm, so every bundle copy shares one slot. In Node/SSR there is a
 * single instance, so the behaviour is unchanged.
 */
declare function setRouteComponentSource(next: RouteComponentSource | undefined): void;
/** The currently registered source, or `undefined` (IR fallback territory). */
declare function getRouteComponentSource(): RouteComponentSource | undefined;
/**
 * True when a path segment is a dynamic-route parameter token. Covers every
 * spelling the build emits: `[name]` and `[...rest]` (the scanner keeps the
 * bracket dir token verbatim), `_name_` (the manifest's normalized form), and
 * `:name` (hand-written route tables). Shared by the {@link routeIdFor}
 * matcher and boot.ts's loading-map matcher (§8.4).
 */
declare function isParamSegment(seg: string): boolean;
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
declare function routeIdFor(routeRef: unknown, manifest: ClientManifestLike): string | undefined;
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
declare function layoutChunksFor(routeId: string, manifest: ClientManifestLike): string[];
/**
 * Extract the page component export from a dynamically imported route chunk
 * module: `default` first, then the conventional named exports, then the
 * module itself when it is a plain component function. Returns `undefined`
 * when nothing component-shaped is found (callers fall back to IR).
 */
declare function pageComponentExport(mod: unknown): unknown;
/**
 * Manifest-driven `componentFor(routeId)` (§8.4): resolve the route's
 * manifest entry, dynamically import its client entry chunk through the
 * registered {@link RouteComponentSource.loadChunk}, and return the page
 * component export. Returns `undefined` (never throws) when no source is
 * registered, the route isn't in the manifest, the chunk fails to load, or
 * the module carries no component export — every caller must treat that as
 * "fall back to the IR renderer".
 */
declare function componentFor(routeRef: string): Promise<unknown>;
export { getRouteComponentSource as a, pageComponentExport as c, componentFor as i, routeIdFor as l, RouteComponentSource as n, isParamSegment as o, RouteManifestEntry as r, layoutChunksFor as s, ClientManifestLike as t, setRouteComponentSource as u };
//# sourceMappingURL=route-manifest-CpS1nShA.d.ts.map