/** Extract width descriptors (`400w`, `800w`, …) from a srcset, sorted
 *  descending, with duplicates removed. Returns `[]` for a srcset that
 *  uses no width descriptors (e.g. `1x`/`2x` density descriptors, or a
 *  plain URL). */
export declare function widthsFromSrcset(srcset: string): number[];
/** The `sizes` hint to emit when the author did not pass one.
 *
 *  For widths `[1200, 800, 400]` the output is:
 *
 *    (min-width: 1200px) 1200px, (min-width: 800px) 800px,
 *    (min-width: 400px) 400px, 100vw
 *
 *  Every candidate gets a `(min-width: Wpx) Wpx` entry so the browser
 *  can pick the narrowest one that still covers the viewport at each
 *  breakpoint. The trailing `100vw` covers viewports narrower than the
 *  smallest candidate.
 *
 *  Returns `undefined` when `srcset` carries no width descriptors —
 *  the caller then leaves `sizes` absent. */
export declare function deriveSizesFromSrcset(srcset: string | undefined): string | undefined;
//# sourceMappingURL=image-sizes.d.ts.map