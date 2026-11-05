<script>
  // HMR fixture: the e2e suite rewrites the marker text in place and watches
  // the DOM update without a navigation. The counter pins the remount
  // semantics of component-level HMR (state resets, DOM updates in place).
  let count = $state(0)
</script>

<section>
  <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
  <button data-hmr-counter="" type="button" onclick={() => (count += 1)}>{count}</button>
</section>
