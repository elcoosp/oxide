<script>
  // `<Island>` — the Svelte adapter's island wrapper (§10.3, §V2-F01).
  //
  // The twin of `packages/react/src/island.ts`: a page writes
  //
  //   <Island component="Counter" chunk="/islands/counter.svelte" props={{ initial: 3 }}>
  //     <Counter initial={3} />
  //   </Island>
  //
  // The component emits an `<ox-island>` custom element whose attribute names
  // and value shapes match `render::island_element()` byte-for-byte (Svelte
  // SSR renders attributes with `"`). The children snippet renders as normal
  // SSR; on the client, the boot script scans `<ox-island[data-ox-chunk]>`,
  // decodes `data-ox-props`, dynamic-imports the chunk and calls the
  // adapter's `hydrateIsland`.
  //
  // §V2-F01 item 4: a FOREIGN island (the `framework` prop naming another
  // framework) emits the self-contained pending placeholder the worker's
  // `island:render` op + the core `resolveForeign` transform resolve with the
  // island's own adapter — no chunk/strategy attrs, children ignored (a
  // foreign framework's children cannot be SSR'd by this adapter).
  import { encodePropsB64 } from '@oxide-fw/core/odf-encode'

  let { component, chunk, strategy = 'load', props = {}, framework, children } = $props()

  // Hydration timing (§10.3): `load`, `idle`, `visible`, or
  // `{ media: '(...)' }` for a CSS media query. Accept both
  // `'(min-width: 800px)'` (the CSS reading) and `'min-width: 800px'`
  // (no parens); the wire shape is always `media(<inner>)`.
  function strategyAttr(s) {
    if (typeof s === 'string') return s
    const inner = s.media.startsWith('(') && s.media.endsWith(')')
      ? s.media.slice(1, -1)
      : s.media
    return `media(${inner})`
  }

  const propsB64 = encodePropsB64(props)
  const foreign = framework !== undefined && framework !== 'svelte'
</script>

{#if foreign}
  <ox-island data-ox-island={component} data-ox-fw={framework} data-ox-pending="1" data-ox-props={propsB64}></ox-island>
{:else}
  <ox-island data-ox-island={component} data-ox-chunk={chunk} data-ox-strategy={strategyAttr(strategy)} data-ox-props={propsB64}>{@render children?.()}</ox-island>
{/if}
