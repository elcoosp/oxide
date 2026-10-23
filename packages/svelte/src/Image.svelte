<!--
  `<Image>` — responsive image component (§16.1), Svelte adapter.

  A Svelte consumer imports this directly:

    import Image from '@oxide/svelte/image'
    <Image {...hero} alt="Hero" sizes="100vw" priority />

  where `hero` is a `?w=` variant module (`{ src, srcset, width, height }`).

  A `.svelte` file is the only thing a Svelte template can render as
  `<Image … />` — a runtime JS function cannot be called from markup with
  props in a way the compiler understands. The consumer's own Svelte
  compiler handles this file; the package ships the source, not a
  precompiled artifact, so the consumer's Svelte version applies.
-->
<script>
  let {
    src,
    srcset = undefined,
    sizes = undefined,
    width = undefined,
    height = undefined,
    alt,
    priority = false,
    lazy = undefined,
    class: cls = undefined,
    ...rest
  } = $props()

  const loading = $derived(lazy ?? !priority ? 'lazy' : 'eager')

  /** Mirror of packages/oxide/src/image-sizes.ts, inlined for the same
   *  reason Defer.svelte inlines its shape check: this .svelte file is
   *  compiled by the consumer's compiler, and its emitted module sits
   *  wherever the consumer's build puts it — a relative import to the
   *  source would break. Keep this in sync with the shared helper. */
  function deriveSizesFromSrcset(srcset) {
    if (!srcset) return undefined
    const widths = []
    for (const part of srcset.split(',')) {
      const m = /(?:\s|^)(\d+)w$/.exec(part.trim())
      if (m) {
        const w = Number(m[1])
        if (Number.isFinite(w) && w > 0) widths.push(w)
      }
    }
    const unique = [...new Set(widths)].sort((a, b) => b - a)
    if (unique.length === 0) return undefined
    const parts = unique.map((w) => `(min-width: ${w}px) ${w}px`)
    parts.push('100vw')
    return parts.join(', ')
  }

  const effectiveSizes = $derived(sizes ?? deriveSizesFromSrcset(srcset))
</script>

<svelte:head>
  {#if priority}
    <link rel="preload" as="image" href={src} imagesrcset={srcset} imagesizes={effectiveSizes} />
  {/if}
</svelte:head>

<img
  {src}
  {srcset}
  {effectiveSizes}
  {width}
  {height}
  {alt}
  {loading}
  decoding="async"
  class={cls}
  fetchpriority={priority ? 'high' : undefined}
  {...rest}
/>
