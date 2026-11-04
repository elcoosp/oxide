export interface SwapDetail {
  id: number;
  /**
   * The **streamed HTML content** of a `suspense: 'core'` boundary — the
   * `innerHTML` of the `<template data-ox-c='<id>'>` the server emitted.
   * `render::SWAPPER_SRC` sends this as `detail.value` on every `ox:swap`,
   * because the whole point of a Suspense boundary is "show this
   * placeholder, then swap in the ready subtree." That subtree is
   * **rendered markup**, not the raw data value.
   *
   * Separate mechanism, same event slot: `defer()` values in *loader data*
   * (§9.2) arrive via `<script type="application/oxide-deferred"
   * data-id="<id>">` tags. Those carry the **structured** JSON value
   * (`{"label":"fast"}`) and flow through `resolveFromScripts`, not this
   * `SwapDetail` path. That's why a `DeferredClient` awaiting a loader
   * slot resolves to the typed value, while one awaiting a Suspense
   * boundary resolves to the boundary's rendered HTML — two shapes, two
   * protocols, both §7.1.
   */
  value?: string;
  error?: unknown;
}
/** A promise-like the client adapter can await; resolves on ox:swap. */
export declare class DeferredClient<T = unknown> {
  #private;
  status: 'pending' | 'resolved' | 'rejected';
  value?: T;
  error?: unknown;
  readonly promise: Promise<T>;
  constructor();
  resolve(value: T): void;
  reject(error: unknown): void;
  then: <R1 = T, R2 = never>(onOk?: ((v: T) => R1 | PromiseLike<R1>) | null, onErr?: ((e: unknown) => R2 | PromiseLike<R2>) | null) => Promise<R1 | R2>;
  catch: <R = never>(onErr?: ((e: unknown) => R | PromiseLike<R>) | null) => Promise<T | R>;
}
export interface DeferGlue {
  /** Rewrite `{__deferred: id}` placeholders in place → DeferredClient. */
  wire(data: unknown): void;
  /** Resolve the slot with a value (e.g. an ODF payload from the stream). */
  resolve(id: number, value: unknown): void;
  /**
   * §7.1 / §9.2: resolve a batch of already-resolved values (the `/__data`
   * envelope's `deferred` map). A client navigation carries the loader's
   * resolved `defer()` output; this settles the matching slots so a
   * component awaiting one gets the value instead of hanging. Unknown ids
   * are ignored (a slot may not be wired yet).
   */
  resolveAll(values: Record<string, unknown>): void;
  reject(id: number, error: unknown): void;
  readonly registry: Map<number, DeferredClient>;
  dispose(): void;
}
/** Structural interface for a "script-like" element the glue reads. The
 *  glue only ever uses `.getAttribute` and `.textContent`, so a real
 *  `HTMLScriptElement` structurally satisfies this — no cast needed. */
export interface DeferGlueElement {
  getAttribute?(name: string): string | null;
  textContent?: string | null;
}
/** Structural interface for the "document-like" object the glue attaches to.
 *  Real `Document` satisfies this. A test fake document also satisfies it
 *  (the previously-required `Document` type forced every caller to cast). */
export interface DeferGlueDoc {
  addEventListener(type: string, fn: (ev: unknown) => void): void;
  removeEventListener(type: string, fn: (ev: unknown) => void): void;
  querySelectorAll?(sel: string): ArrayLike<DeferGlueElement>;
  documentElement?: unknown;
  readyState?: string;
}
export interface DeferGlueOptions {
  doc?: DeferGlueDoc;
  /** Called on every ox:swap (adapter hydration of swapped content, §7.1). */
  onSwap?(detail: SwapDetail): void;
  /** Custom event name (default 'ox:swap'). */
  eventName?: string;
}
export declare function attachDeferGlue(options?: DeferGlueOptions): DeferGlue;
export interface WireOptions {
  registry?: Map<number, DeferredClient>;
  /** Dead glue: newly wired slots reject immediately instead of hanging. */
  dead?: boolean;
}
/**
 * Walk `data` (which came from the ODF decoder) and replace every
 * `{__deferred: id}` placeholder with a [DeferredClient] registered under its
 * id. Mutates containers in place; cycle-safe; depth-capped like the decoder.
 */
export declare function wireDeferreds(data: unknown, options?: WireOptions): unknown;
//# sourceMappingURL=client-defer-glue.d.ts.map