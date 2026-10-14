<script setup lang="ts">
// Loader data arrives as `props.data` — same contract every adapter uses.
defineProps<{ data: { message: string } }>()
</script>

<template>
  <main>
    <h1>Home</h1>
    <p>{{ data.message }}</p>
  </main>
</template>
