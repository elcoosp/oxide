/** Structural island host (real element or test fake). */
export interface IslandHost {
  getAttribute(name: string): string | null;
  hasAttribute(name: string): boolean;
  setAttribute?(name: string, value: string): void;
}
interface IslandsDoc {
  querySelectorAll(sel: string): ArrayLike<IslandHost>;
}
export interface IslandsAdapter {
  /** Synchronous (IR replay) or asynchronous (React's `react-dom/client`,
   *  which code-splits its runtime). The caller awaits either. */
  hydrateIsland?(el: Element, mod: unknown, props: unknown): (() => void) | Promise<() => void>;
  hydrate?(root: Element, input: unknown): () => void;
}
export interface IslandsOptions {
  adapter: IslandsAdapter;
  doc?: IslandsDoc;
  /** Load a chunk reference (dynamic import in the browser). */
  loadChunk?: (chunk: string) => Promise<unknown>;
  requestIdleCallback?: (cb: () => void) => unknown;
  matchMedia?: (query: string) => {
    matches: boolean;
    addEventListener?(type: string, cb: () => void): void;
    removeEventListener?(type: string, cb: () => void): void;
  };
  IntersectionObserver?: new (cb: (entries: Array<{
    target: unknown;
    isIntersecting: boolean;
  }>) => void) => {
    observe(el: unknown): void;
    unobserve(el: unknown): void;
  };
}
export type ParsedStrategy = {
  kind: 'load' | 'idle' | 'visible';
} | {
  kind: 'media';
  query: string;
};
/** Parse `data-ox-strategy`: `load | idle | visible | media(query)`. */
export declare function parseStrategy(raw: string | null): ParsedStrategy;
/** Decode `data-ox-props`: base64 ODF (§10.3), with a JSON fallback for
 * payloads that predate the ODF props encoding (render test fixtures). */
export declare function decodeIslandProps(b64: string): unknown;
export interface IslandCtx extends IslandsOptions {
  loadChunk: (chunk: string) => Promise<unknown>;
}
/** Hydrate one island now: decode props → load chunk → adapter dispatch.
 *
 * Two attributes bracket the work so consumers (tests, devtools, a
 * readiness check) can distinguish "hydration started" from "hydration
 * finished":
 *   * `data-ox-hydrated="pending"` — set synchronously on entry, serves as
 *     the double-hydration guard.
 *   * `data-ox-hydrated="done"` — set after the adapter's `hydrateIsland`
 *     returns, i.e. once framework listeners are attached. A browser test
 *     that needs to click the island waits for this.
 *   * `data-ox-hydrated="error"` — the adapter threw; the island stays
 *     inert and the attribute records why.
 */
export declare function hydrateIsland(el: IslandHost, ctx: IslandCtx): Promise<(() => void) | undefined>;
/**
 * Schedule every `<ox-island[data-ox-chunk]>` under `doc` by its strategy
 * (§10.3): `load` → now; `idle` → requestIdleCallback (setTimeout fallback);
 * `visible` → IntersectionObserver; `media(q)` → matchMedia now/change.
 * Returns a disposer that cancels pending schedules.
 */
export declare function hydrateIslands(opts: IslandsOptions): () => void;
export {};
//# sourceMappingURL=client-islands.d.ts.map