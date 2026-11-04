export interface BoundaryRegistration {
  /** The id the placeholder carried, matching `{ __deferred: id }`. */
  id: number;
  /** A component that renders the resolved content standalone. */
  Component: unknown;
  /** Props for that component (the resolved value lives here). */
  props: Record<string, unknown>;
}
type Slot = BoundaryRegistration[];
declare global {
  var __oxide_boundary_registry: BoundaryRegistration[] | undefined;
}
/**
 * Start capturing boundaries. Returns the previous slot so a nested render
 * (an island, an error boundary) restores it instead of clobbering the
 * outer capture.
 *
 * Safe as a process-global because the worker serialises calls into one
 * module instance — the same assumption `__oxide_fn_registry` makes.
 */
export declare function beginBoundaryCapture(): Slot | undefined;
/** Restore the slot captured by `beginBoundaryCapture`. */
export declare function endBoundaryCapture(prev: Slot | undefined): void;
/** Take everything captured so far. Leaves the slot empty for the next batch. */
export declare function drainBoundaries(): BoundaryRegistration[];
/** Called by `<Defer>` while the shell is rendering. A no-op outside a
 *  capture: a component rendered standalone still emits its placeholder. */
export declare function registerBoundary(entry: BoundaryRegistration): void;
/** The `{ __deferred: id }` placeholder a loader's data carries, or null. */
export declare function deferredIdOf(value: unknown): number | null;
export declare const BOUNDARY_FALLBACK_ATTR = "data-ox-b";
export declare const BOUNDARY_CONTENT_ATTR = "data-ox-c";
declare function escapeText(s: string): string;
declare function escapeAttr(s: string): string;
/** Attribute-only selectors so one swapper drives both host shapes: the IR
 *  renderer's `<template>` and a framework's `<ox-defer>`. */
export declare const SWAPPER_SRC: string;
export declare function boundaryFallback(id: number, fallbackHtml: string): string;
export declare function boundaryContent(id: number, html: string, nonce?: string): string;
export { escapeAttr, escapeText };
//# sourceMappingURL=server-boundary.d.ts.map