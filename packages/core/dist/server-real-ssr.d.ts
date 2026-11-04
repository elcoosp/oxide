import { a as FrameworkAdapter, i as Disposer, o as HeadTags, t as ClientInput, u as RenderInput } from "./types-CoCogOdY.js";
import { resolveDeferredPlaceholders } from "./server-deferred.js";
export interface RealAdapterOptions {
  /**
   * Which renderer the adapter uses.
   * - `'auto'` (default): prefer real SSR, fall back to IR when the caller
   *   has no component for a route OR the framework deps are unavailable.
   * - `'real'`: require real SSR; missing inputs render an HTML comment
   *   placeholder instead of falling back (useful for production).
   */
  ssr?: 'real' | 'auto';
  /**
   * Advanced override: resolve a component for a route id without the
   * generated resolver. The normal path is {@link RenderInput.resolve},
   * supplied by the runtime from the build-emitted component surface
   * (§6.4). Kept for one release window for custom component registries.
   */
  componentFor?: (route: string) => unknown;
  /** Optional layout: called with the route's tree as children (React-style). */
  layoutFor?: (route: string) => unknown;
}
export interface RealAdapterConfig {
  name: string;
  extensions: string[];
  jsxImportSource?: string;
  capabilities: FrameworkAdapter['capabilities'];
  plugins: () => ReturnType<FrameworkAdapter['plugins']>;
  /** IR fallback render (from the adapter's ./render.ts). */
  irRender: (input: RenderInput) => ReadableStream<Uint8Array>;
  /**
   * Final HeadTags for a RenderInput: the adapter composes its own layout
   * head, `head(ctx)` output, and the pipeline-supplied `input.head`. The
   * helper only emits the resulting model.
   */
  buildHead: (input: RenderInput) => HeadTags;
  /** True when the upstream framework deps are resolvable. */
  isAvailable: () => Promise<boolean>;
  /** Per-adapter load failure detail (BUG-09): set from the adapter's own
   * `isAvailable()` catch — the generic helper must not read a
   * React-specific global for all four frameworks. */
  lastError?: () => string | undefined;
  /** Real SSR body: given a component and props, produce HTML. */
  renderBody: (Component: unknown, props: Record<string, unknown>) => Promise<string>;
  /**
   * §7.1 streaming SSR: an optional chunked body renderer. When present, the
   * adapter yields its stream's chunks as the response body — the shell
   * reaches the browser before deferred boundaries resolve. Absent → the
   * adapter uses `renderBody` (one chunk). React wires this to React 19's
   * `renderToReadableStream`; the others can adopt it as their frameworks
   * expose a streaming renderer.
   */
  renderStream?: (Component: unknown, props: Record<string, unknown>) => Promise<ReadableStream<Uint8Array>>;
  /** Attributes on the framework root container (e.g. ` id="root" data-reactroot=""`). */
  rootAttrs: string;
  /** Client-side entries — re-used from the IR adapter (framework-neutral). */
  hydrate: FrameworkAdapter['hydrate'];
  hydrateIsland?: FrameworkAdapter['hydrateIsland'];
  mount: FrameworkAdapter['mount'];
  update: FrameworkAdapter['update'];
  opts: RealAdapterOptions;
}
/** Render a neutral HeadTags model to the strings that go inside <head>. */
export declare function emitHead(head: HeadTags): string;
export declare function makeRealAdapter(cfg: RealAdapterConfig): FrameworkAdapter;
export { type ClientInput, type Disposer, resolveDeferredPlaceholders };
//# sourceMappingURL=server-real-ssr.d.ts.map