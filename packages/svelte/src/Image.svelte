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
</script>

<svelte:head>
  {#if priority}
    <link rel="preload" as="image" href={src} imagesrcset={srcset} imagesizes={sizes} />
  {/if}
</svelte:head>

<img
  {src}
  {srcset}
  {sizes}
  {width}
  {height}
  {alt}
  {loading}
  decoding="async"
  class={cls}
  fetchpriority={priority ? 'high' : undefined}
  {...rest}
/>
