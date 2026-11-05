<script setup lang="ts">
import { ref } from 'vue'

// HMR fixture: the e2e suite rewrites the marker text in place and watches
// the DOM update without a navigation. The counter pins the remount
// semantics of component-level HMR (state resets, DOM updates in place).
const count = ref(0)
</script>

<template>
  <section>
    <h2 data-hmr-target="">HMR-TARGET-ORIGINAL</h2>
    <button data-hmr-counter="" type="button" @click="count++">{{ count }}</button>
  </section>
</template>
