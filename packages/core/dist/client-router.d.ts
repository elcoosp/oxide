import { t as ClientManifestLike } from "./route-manifest-CpS1nShA.js";
import { a as FrameworkAdapter, t as ClientInput } from "./types-CoCogOdY.js";
export interface RouterDoc {
  addEventListener(type: string, fn: (e: unknown) => void, opts?: AddEventListenerOptions): void;
  removeEventListener(type: string, fn: (e: unknown) => void, opts?: AddEventListenerOptions): void;
  querySelectorAll(sel: string): ArrayLike<RouterLink>;
  startViewTransition?(cb: () => void): {
    finished: Promise<void>;
  };
}
export interface RouterLink {
  getAttribute(name: string): string | null;
  hasAttribute(name: string): boolean;
  closest(sel: string): RouterLink | null;
}
interface HistoryLike {
  state: unknown;
  pushState(state: unknown, title: string, url: string): void;
  replaceState(state: unknown, title: string, url: string): void;
}
interface LocationLike {
  href: string;
  pathname: string;
  search: string;
}
export interface RouterOptions {
  adapter: Pick<FrameworkAdapter, 'update'>;
  /** Unused today (SPA nav fetches `/__data`). Reserved for manifest-driven
   *  chunk preloading (§6.4); see WORKLOG. */
  manifest?: ClientManifestLike;
  doc?: RouterDoc;
  history?: HistoryLike;
  location?: LocationLike;
  fetch?: typeof fetch;
  scrollTo?: (x: number, y: number) => void;
  getScroll?: () => {
    x: number;
    y: number;
  };
  IntersectionObserver?: (new (cb: (entries: Array<{
    target: unknown;
    isIntersecting: boolean;
  }>) => void) => {
    observe(el: unknown): void;
    disconnect(): void;
  }) | undefined;
  /** Concurrent prefetch cap (§8.4 "budgeted and cancelable"). Default 5. */
  prefetchBudget?: number;
  /**
   * §6.4: the URL prefix the client bundle is served from. The manifest's
   * chunk values are bare filenames; the router prefixes them with this so a
   * preload `import()` resolves. Defaults to `import.meta.url`'s directory
   * (`/client/` in the built entry), falling back to `/client/`.
   */
  clientBase?: string;
  /**
   * §8.4 "optimistic navigation with loading boundaries": fired as soon as
   * `goto()` starts — before the `/__data` fetch — with a best-effort input
   * whose `route` is the currently-known path (the resolved route id is not
   * available yet; the framework resolves the loading component from the
   * manifest keyed on the URL's nearest route base). The hook is
   * **synchronous**; the caller renders a loading state and returns nothing.
   * Paired with {@link onNavigateEnd}, which fires after the data resolves
   * and just before `adapter.update` swaps the tree.
   */
  onNavigateStart?: (url: string) => void;
  /** Fired after the `/__data` fetch resolves and just before `adapter.update`
   *  commits the new tree. The caller tears down whatever {@link
   *  onNavigateStart} rendered. Synchronous. */
  onNavigateEnd?: (input: ClientInput) => void;
}
export interface Router {
  attach(): () => void;
  dispose(): void;
  goto(href: string, opts?: {
    replace?: boolean;
  }): Promise<void>;
  prefetch(href: string): void;
  current(): ClientInput | undefined;
}
export declare function createRouter(opts: RouterOptions): Router;
export {};
//# sourceMappingURL=client-router.d.ts.map