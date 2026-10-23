<script>
  let { value = undefined, children, fallback } = $props()

  function deferredIdOf(v) {
    if (v === null || typeof v !== 'object') return null
    if (typeof v.__deferred !== 'number') return null
    if (Object.keys(v).length !== 1) return null
    return v.__deferred
  }

  const id = deferredIdOf(value)
</script>

{#if id === null}
  {#if children}{@render children({ value })}{/if}
{:else}
  <ox-defer data-ox-b={String(id)}>
    {#if fallback}{@render fallback()}{/if}
  </ox-defer>
{/if}
