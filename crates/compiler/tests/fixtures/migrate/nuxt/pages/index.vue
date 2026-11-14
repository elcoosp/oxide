<script setup>
const posts = await useFetch('/api/posts')
</script>
<template><ul><li v-for="p in posts" :key="p.id">{{ p.title }}</li></ul></template>
