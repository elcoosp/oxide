// §16.1: `sizes` auto-generation from a `srcset` width ladder.
//
// An `<img srcset="…w, …w">` without `sizes` defaults to `100vw` in
// the browser — the browser picks the candidate wide enough to cover
// the full viewport. That's correct but wasteful: on a 1440px monitor
// the browser requests the widest candidate even if the image sits in
// a 640px column.
//
// This helper derives a `sizes` hint from the widths already in the
// `srcset`: a `(min-width: Wpx) Wpx` entry per candidate, descending,
// with `100vw` as the terminal fallback so a viewport narrower than the
// smallest candidate still resolves. The output is what a human would
// write by hand for a "scale up to intrinsic width" layout.
//
// Consumers (the four adapter `<Image>` components) call this only when
// `sizes` is absent. An explicit `sizes` is never overridden.

/** Extract width descriptors (`400w`, `800w`, …) from a srcset, sorted
 *  descending, with duplicates removed. Returns `[]` for a srcset that
 *  uses no width descriptors (e.g. `1x`/`2x` density descriptors, or a
 *  plain URL). */
export function widthsFromSrcset(srcset: string): number[] {
  const widths: number[] = []
  for (const part of srcset.split(',')) {
    // `<url> <n>w` — the descriptor is the last whitespace-separated
    // token, per the srcset grammar.
    const m = /(?:\s|^)(\d+)w$/.exec(part.trim())
    if (m) {
      const w = Number(m[1])
      if (Number.isFinite(w) && w > 0) widths.push(w)
    }
  }
  return [...new Set(widths)].sort((a, b) => b - a)
}

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
export function deriveSizesFromSrcset(srcset: string | undefined): string | undefined {
  if (!srcset) return undefined
  const widths = widthsFromSrcset(srcset)
  if (widths.length === 0) return undefined
  const parts = widths.map((w) => `(min-width: ${w}px) ${w}px`)
  parts.push('100vw')
  return parts.join(', ')
}
