import { t as ClientManifestLike } from "./route-manifest-CpS1nShA.js";
import { a as FrameworkAdapter } from "./types-CoCogOdY.js";
/**
 * Register an adapter and its chunk loader, then run the island scan. The
 * generated boot entry calls this after its static imports have resolved.
 * Safe to call multiple times: the scan is idempotent because each
 * `<ox-island>` host carries a `data-ox-hydrated` guard after its first
 * hydration attempt.
 *
 * Runs synchronously if the document is already interactive; otherwise
 * waits for `DOMContentLoaded`.
 */
export declare function boot(): void;
/**
 * Boot the client router (§8.4): hydrate the SSR root from `window.__oxide`,
 * then attach `createRouter` so same-origin link clicks navigate via
 * `/__data` instead of a full page load.
 *
 * Guarded and idempotent:
 * - no `document` → no-op;
 * - no `window.__oxide` → islands + scripts only, no router (plain anchors);
 * - `window.__oxide` with `#root` → full mode: hydrate the root, islands,
 *   then the router. A hydration FAILURE leaves the SSR markup untouched and
 *   does NOT attach the router (the adapters' `update` would no-op and the
 *   URL would change without a DOM swap — worse than plain anchors);
 * - `window.__oxide` WITHOUT `#root` → router-only mode: nothing to
 *   hydrate, but the page still gets the loading overlays (§8.4), the defer
 *   glue (§7.1/§9.2), manifest-driven chunk preloading (§6.4) and the
 *   `ox:data` HMR path (§13.3). The adapters' `update` calls no-op without
 *   a mounted root.
 * - `data` in `window.__oxide` is base64 ODF, decoded via `decodeOdf`; a
 *   decode failure falls back to the raw string rather than throwing.
 *
 * Returns a disposer (the router's `dispose` + boot glue teardown), or a
 * no-op.
 */
/** The minimum adapter surface `bootWithRouter` needs. A superset of what
 *  `IslandsAdapter` and `RouterOptions.adapter` require: `hydrate` for the
 *  root, `update` for the router, optional `hydrateIsland`/`renderLoading`
 *  for the island and loading paths. The full `FrameworkAdapter` satisfies
 *  this structurally — adapter authors keep using `FrameworkAdapter`, and
 *  test fixtures only need the fields they actually exercise. */
export type BootAdapter = Pick<FrameworkAdapter, 'hydrate' | 'update'> & Partial<Pick<FrameworkAdapter, 'hydrateIsland' | 'renderLoading'>>;
export declare function bootWithRouter(adapter: BootAdapter, opts?: {
  doc?: Document;
  /**
   * §8.4 loading boundaries: route-pattern → loading component map. Keys
   * are absolute paths whose segments may be `[name]` / `:name` tokens for
   * dynamic segments (`/blog/[slug]`), emitted by the boot entry from each
   * `loading.tsx`'s route directory. On a client navigation the longest
   * matching key (most matching segments; a key may cover the routes below
   * it) wins and `adapter.renderLoading` paints it into an overlay until
   * the tree swaps. Absent (or a framework without `renderLoading`) → no
   * loading boundary; the router still fires the hooks, they are just
   * no-ops here.
   */
  loading?: Record<string, unknown>;
  /**
   * §6.4 manifest-driven chunk preloading + §8.4/§10.3 real component
   * resolution: the wire shape of `/client/client.manifest.json`
   * (`{ buildId, routes: { [routeId]: { entry_chunk, preload, css } } }`).
   * The router warms a route's chunks in parallel with the `/__data`
   * fetch, and the manifest is registered as the adapters'
   * {@link setRouteComponentSource | component source} so `update()`
   * renders the route's REAL page component instead of the IR registry.
   * Absent → data-only prefetch and the IR fallback.
   */
  clientBase?: string;
  manifest?: ClientManifestLike & {
    /**
     * §16.4: the locales this build emitted catalogs for. Absent when the
     * project has no i18n — `setMessageLocales(undefined)` then leaves the
     * fetch enabled (the pre-existing behaviour); an empty array would too,
     * so the CLI omits the field rather than sending `[]`.
     */
    i18n_locales?: string[];
  };
}): () => void;
/**
 * Install the boot runtime on `window` without running the island scan.
 * Called by the generated entry before `boot()` so `__oxideRegisterAdapter`
 * is available when the entry imports the framework adapter. No-op outside
 * the browser — safe to call from any module.
 */
export declare function installBootRuntime(): void;
//# sourceMappingURL=client-boot.d.ts.map